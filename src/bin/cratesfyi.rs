use std::env;
use std::fmt::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context as _, Error, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use docs_rs::cdn::CdnBackend;
use docs_rs::db::{self, CrateId, Overrides, Pool, add_path_into_database};
use docs_rs::repositories::RepositoryStatsUpdater;
use docs_rs::utils::{
    ConfigName, get_config, get_crate_pattern_and_priority, list_crate_priorities, queue_builder,
    remove_crate_priority, set_config, set_crate_priority,
};
use docs_rs::{
    AsyncBuildQueue, AsyncStorage, BuildQueue, Config, Context, Index, InstanceMetrics,
    PackageKind, RegistryApi, RustwideBuilder, ServiceMetrics, Storage,
    start_background_metrics_webserver, start_web_server,
};
use futures_util::StreamExt;
use humantime::Duration;
use once_cell::sync::OnceCell;
use sentry::{
    TransactionContext, integrations::panic as sentry_panic,
    integrations::tracing as sentry_tracing,
};
use tokio::runtime::{Builder, Runtime};
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, filter::Directive, prelude::*};

fn main() {
    // set the global log::logger for backwards compatibility
    // through rustwide.
    rustwide::logging::init_with(LogTracer::new());

    let log_formatter = {
        let log_format = env::var("DOCSRS_LOG_FORMAT").unwrap_or_default();

        if log_format == "json" {
            tracing_subscriber::fmt::layer().json().boxed()
        } else {
            tracing_subscriber::fmt::layer().boxed()
        }
    };

    let tracing_registry = tracing_subscriber::registry().with(log_formatter).with(
        EnvFilter::builder()
            .with_default_directive(Directive::from_str("docs_rs=info").unwrap())
            .with_env_var("DOCSRS_LOG")
            .from_env_lossy(),
    );

    let _sentry_guard = if let Ok(sentry_dsn) = env::var("SENTRY_DSN") {
        tracing::subscriber::set_global_default(tracing_registry.with(
            sentry_tracing::layer().event_filter(|md| {
                if md.fields().field("reported_to_sentry").is_some() {
                    sentry_tracing::EventFilter::Ignore
                } else {
                    sentry_tracing::default_event_filter(md)
                }
            }),
        ))
        .unwrap();

        let traces_sample_rate = env::var("SENTRY_TRACES_SAMPLE_RATE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);

        let traces_sampler = move |ctx: &TransactionContext| -> f32 {
            if let Some(sampled) = ctx.sampled() {
                // if the transaction was already marked as "to be sampled" by
                // the JS/frontend SDK, we want to sample it in the backend too.
                return if sampled { 1.0 } else { 0.0 };
            }

            let op = ctx.operation();
            if op == "docbuilder.build_package" {
                // record all transactions for builds
                1.
            } else {
                traces_sample_rate
            }
        };

        Some(sentry::init((
            sentry_dsn,
            sentry::ClientOptions {
                release: Some(docs_rs::BUILD_VERSION.into()),
                attach_stacktrace: true,
                traces_sampler: Some(Arc::new(traces_sampler)),
                ..Default::default()
            }
            .add_integration(sentry_panic::PanicIntegration::default()),
        )))
    } else {
        tracing::subscriber::set_global_default(tracing_registry).unwrap();
        None
    };

    if let Err(err) = CommandLine::parse().handle_args() {
        let mut msg = format!("Error: {err}");
        for cause in err.chain() {
            write!(msg, "\n\nCaused by:\n    {cause}").unwrap();
        }
        eprintln!("{msg}");

        let backtrace = err.backtrace().to_string();
        if !backtrace.is_empty() {
            eprintln!("\nStack backtrace:\n{backtrace}");
        }

        // we need to drop the sentry guard here so all unsent
        // errors are sent to sentry before
        // process::exit kills everything.
        drop(_sentry_guard);
        std::process::exit(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
enum Toggle {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = docs_rs::BUILD_VERSION,
    rename_all = "kebab-case",
)]
enum CommandLine {
    Build {
        #[command(subcommand)]
        subcommand: BuildSubcommand,
    },

    /// Starts web server
    StartWebServer {
        #[arg(name = "SOCKET_ADDR", default_value = "0.0.0.0:3000")]
        socket_addr: SocketAddr,
    },

    StartRegistryWatcher {
        #[arg(name = "SOCKET_ADDR", default_value = "0.0.0.0:3000")]
        metric_server_socket_addr: SocketAddr,
        /// Enable or disable the repository stats updater
        #[arg(
            long = "repository-stats-updater",
            default_value = "disabled",
            value_enum
        )]
        repository_stats_updater: Toggle,
        #[arg(long = "cdn-invalidator", default_value = "enabled", value_enum)]
        cdn_invalidator: Toggle,
        #[arg(long = "queue-rebuilds", default_value = "enabled", value_enum)]
        queue_rebuilds: Toggle,
    },

    StartBuildServer {
        #[arg(name = "SOCKET_ADDR", default_value = "0.0.0.0:3000")]
        metric_server_socket_addr: SocketAddr,
    },

    /// Starts the daemon
    Daemon {
        /// Enable or disable the registry watcher to automatically enqueue newly published crates
        #[arg(long = "registry-watcher", default_value = "enabled", value_enum)]
        registry_watcher: Toggle,
    },

    /// Database operations
    Database {
        #[command(subcommand)]
        subcommand: DatabaseSubcommand,
    },

    /// Interactions with the build queue
    Queue {
        #[command(subcommand)]
        subcommand: QueueSubcommand,
    },
}

impl CommandLine {
    fn handle_args(self) -> Result<()> {
        let ctx = BinContext::new();

        match self {
            Self::Build { subcommand } => subcommand.handle_args(ctx)?,
            Self::StartRegistryWatcher {
                metric_server_socket_addr,
                repository_stats_updater,
                cdn_invalidator,
                queue_rebuilds,
            } => {
                if repository_stats_updater == Toggle::Enabled {
                    docs_rs::utils::daemon::start_background_repository_stats_updater(&ctx)?;
                }
                if cdn_invalidator == Toggle::Enabled {
                    docs_rs::utils::daemon::start_background_cdn_invalidator(&ctx)?;
                }
                if queue_rebuilds == Toggle::Enabled {
                    docs_rs::utils::daemon::start_background_queue_rebuild(&ctx)?;
                }

                start_background_metrics_webserver(Some(metric_server_socket_addr), &ctx)?;

                ctx.runtime()?.block_on(async {
                    docs_rs::utils::watch_registry(
                        ctx.async_build_queue().await?,
                        ctx.config()?,
                        ctx.index()?,
                    )
                    .await
                })?;
            }
            Self::StartBuildServer {
                metric_server_socket_addr,
            } => {
                start_background_metrics_webserver(Some(metric_server_socket_addr), &ctx)?;

                let build_queue = ctx.build_queue()?;
                let config = ctx.config()?;
                let rustwide_builder = RustwideBuilder::init(&ctx)?;
                queue_builder(&ctx, rustwide_builder, build_queue, config)?;
            }
            Self::StartWebServer { socket_addr } => {
                // Blocks indefinitely
                start_web_server(Some(socket_addr), &ctx)?;
            }
            Self::Daemon { registry_watcher } => {
                docs_rs::utils::start_daemon(ctx, registry_watcher == Toggle::Enabled)?;
            }
            Self::Database { subcommand } => subcommand.handle_args(ctx)?,
            Self::Queue { subcommand } => subcommand.handle_args(ctx)?,
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum QueueSubcommand {
    /// Add a crate to the build queue
    Add {
        /// Name of crate to build
        #[arg(name = "CRATE_NAME")]
        crate_name: String,
        /// Version of crate to build
        #[arg(name = "CRATE_VERSION")]
        crate_version: String,
        /// Priority of build (new crate builds get priority 0)
        #[arg(
            name = "BUILD_PRIORITY",
            short = 'p',
            long = "priority",
            default_value = "5",
            allow_negative_numbers = true
        )]
        build_priority: i32,
    },

    /// Interactions with build queue priorities
    DefaultPriority {
        #[command(subcommand)]
        subcommand: PrioritySubcommand,
    },

    /// Get the registry watcher's last seen reference
    GetLastSeenReference,

    /// Set the registry watcher's last seen reference
    #[command(arg_required_else_help(true))]
    SetLastSeenReference {
        /// The reference to set to, required unless flag used
        #[arg(conflicts_with("head"))]
        reference: Option<crates_index_diff::gix::ObjectId>,

        /// Fetch the current HEAD of the remote index and use it
        #[arg(long, conflicts_with("reference"))]
        head: bool,
    },
}

impl QueueSubcommand {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        let build_queue = ctx.build_queue()?;
        match self {
            Self::Add {
                crate_name,
                crate_version,
                build_priority,
            } => build_queue.add_crate(
                &crate_name,
                &crate_version,
                build_priority,
                ctx.config()?.registry_url.as_deref(),
            )?,

            Self::GetLastSeenReference => {
                if let Some(reference) = build_queue.last_seen_reference()? {
                    println!("Last seen reference: {reference}");
                } else {
                    println!("No last seen reference available");
                }
            }

            Self::SetLastSeenReference { reference, head } => {
                let reference = match (reference, head) {
                    (Some(reference), false) => reference,
                    (None, true) => {
                        println!("Fetching changes to set reference to HEAD");
                        let (_, oid) = ctx.index()?.diff()?.peek_changes()?;
                        oid
                    }
                    (_, _) => unreachable!(),
                };

                build_queue.set_last_seen_reference(reference)?;
                println!("Set last seen reference: {reference}");
            }

            Self::DefaultPriority { subcommand } => subcommand.handle_args(ctx)?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum PrioritySubcommand {
    /// Get priority for a crate
    ///
    /// (returns only the first matching pattern, there may be other matching patterns)
    Get { crate_name: String },

    /// List priorities for all patterns
    List,

    /// Set all crates matching a pattern to a priority level
    Set {
        /// See https://www.postgresql.org/docs/current/functions-matching.html for pattern syntax
        #[arg(name = "PATTERN")]
        pattern: String,
        /// The priority to give crates matching the given `PATTERN`
        #[arg(allow_negative_numbers = true)]
        priority: i32,
    },

    /// Remove the prioritization of crates for a pattern
    Remove {
        /// See https://www.postgresql.org/docs/current/functions-matching.html for pattern syntax
        #[arg(name = "PATTERN")]
        pattern: String,
    },
}

impl PrioritySubcommand {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        ctx.runtime()?.block_on(async move {
            let mut conn = ctx.pool()?.get_async().await?;
            match self {
                Self::List => {
                    for (pattern, priority) in list_crate_priorities(&mut conn).await? {
                        println!("{pattern:>20} : {priority:>3}");
                    }
                }

                Self::Get { crate_name } => {
                    if let Some((pattern, priority)) =
                        get_crate_pattern_and_priority(&mut conn, &crate_name).await?
                    {
                        println!("{pattern} : {priority}");
                    } else {
                        println!("No priority found for {crate_name}");
                    }
                }

                Self::Set { pattern, priority } => {
                    set_crate_priority(&mut conn, &pattern, priority)
                        .await
                        .context("Could not set pattern's priority")?;
                    println!("Set pattern '{pattern}' to priority {priority}");
                }

                Self::Remove { pattern } => {
                    if let Some(priority) = remove_crate_priority(&mut conn, &pattern)
                        .await
                        .context("Could not remove pattern's priority")?
                    {
                        println!("Removed pattern '{pattern}' with priority {priority}");
                    } else {
                        println!("Pattern '{pattern}' did not exist and so was not removed");
                    }
                }
            }
            Ok(())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum BuildSubcommand {
    /// Builds documentation for a crate
    Crate {
        /// Crate name
        #[arg(name = "CRATE_NAME", requires("CRATE_VERSION"))]
        crate_name: Option<String>,

        /// Version of crate
        #[arg(name = "CRATE_VERSION")]
        crate_version: Option<String>,

        /// Build a crate at a specific path
        #[arg(short = 'l', long = "local", conflicts_with_all(&["CRATE_NAME", "CRATE_VERSION"]))]
        local: Option<PathBuf>,
    },

    /// update the currently installed rustup toolchain
    UpdateToolchain {
        /// Update the toolchain only if no toolchain is currently installed
        #[arg(name = "ONLY_FIRST_TIME", long = "only-first-time")]
        only_first_time: bool,
    },

    /// Adds essential files for the installed version of rustc
    AddEssentialFiles,

    SetToolchain {
        toolchain_name: String,
    },

    /// Locks the daemon, preventing it from building new crates
    Lock,

    /// Unlocks the daemon to continue building new crates
    Unlock,
}

impl BuildSubcommand {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        let build_queue = ctx.build_queue()?;
        let rustwide_builder = || -> Result<RustwideBuilder> { RustwideBuilder::init(&ctx) };

        match self {
            Self::Crate {
                crate_name,
                crate_version,
                local,
            } => {
                let mut builder = rustwide_builder()?;

                if let Some(path) = local {
                    builder
                        .build_local_package(&path)
                        .context("Building documentation failed")?;
                } else {
                    let registry_url = ctx.config()?.registry_url.clone();
                    builder
                        .build_package(
                            &crate_name
                                .with_context(|| anyhow!("must specify name if not local"))?,
                            &crate_version
                                .with_context(|| anyhow!("must specify version if not local"))?,
                            registry_url
                                .as_ref()
                                .map(|s| PackageKind::Registry(s.as_str()))
                                .unwrap_or(PackageKind::CratesIo),
                        )
                        .context("Building documentation failed")?;
                }
            }

            Self::UpdateToolchain { only_first_time } => {
                let rustc_version = ctx.runtime()?.block_on({
                    let pool = ctx.pool()?;
                    async move {
                        let mut conn = pool
                            .get_async()
                            .await
                            .context("failed to get a database connection")?;

                        get_config::<String>(&mut conn, ConfigName::RustcVersion).await
                    }
                })?;
                if only_first_time && rustc_version.is_some() {
                    println!("update-toolchain was already called in the past, exiting");
                    return Ok(());
                }

                rustwide_builder()?
                    .update_toolchain()
                    .context("failed to update toolchain")?;

                rustwide_builder()?
                    .purge_caches()
                    .context("failed to purge caches")?;

                rustwide_builder()?
                    .add_essential_files()
                    .context("failed to add essential files")?;
            }

            Self::AddEssentialFiles => {
                rustwide_builder()?
                    .add_essential_files()
                    .context("failed to add essential files")?;
            }

            Self::SetToolchain { toolchain_name } => {
                ctx.runtime()?.block_on(async move {
                    let mut conn = ctx
                        .pool()?
                        .get_async()
                        .await
                        .context("failed to get a database connection")?;
                    set_config(&mut conn, ConfigName::Toolchain, toolchain_name)
                        .await
                        .context("failed to set toolchain in database")
                })?;
            }

            Self::Lock => build_queue.lock().context("Failed to lock")?,
            Self::Unlock => build_queue.unlock().context("Failed to unlock")?,
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum DatabaseSubcommand {
    /// Run database migration
    Migrate {
        /// The database version to migrate to
        #[arg(name = "VERSION")]
        version: Option<i64>,
    },

    /// temporary command to update the `crates.latest_version_id` field
    UpdateLatestVersionId,

    /// Updates GitHub/GitLab stats for crates.
    UpdateRepositoryFields,

    /// Backfill GitHub/GitLab stats for crates.
    BackfillRepositoryStats,

    /// Updates info for a crate from the registry's API
    UpdateCrateRegistryFields {
        #[arg(name = "CRATE")]
        name: String,
    },

    AddDirectory {
        /// Path of file or directory
        #[arg(name = "DIRECTORY")]
        directory: PathBuf,
    },

    /// Remove documentation from the database
    Delete {
        #[command(subcommand)]
        command: DeleteSubcommand,
    },

    /// Blacklist operations
    Blacklist {
        #[command(subcommand)]
        command: BlacklistSubcommand,
    },

    /// Limit overrides operations
    Limits {
        #[command(subcommand)]
        command: LimitsSubcommand,
    },

    /// Compares the database with the index and resolves inconsistencies
    Synchronize {
        /// Don't actually resolve the inconsistencies, just log them
        #[arg(long)]
        dry_run: bool,
    },
}

impl DatabaseSubcommand {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        match self {
            Self::Migrate { version } => {
                let pool = ctx.pool()?;
                ctx.runtime()?
                    .block_on(async {
                        let mut conn = pool.get_async().await?;
                        db::migrate(&mut conn, version).await
                    })
                    .context("Failed to run database migrations")?
            }

            Self::UpdateLatestVersionId => {
                let pool = ctx.pool()?;
                ctx.runtime()?
                    .block_on(async {
                        let mut list_conn = pool.get_async().await?;
                        let mut update_conn = pool.get_async().await?;

                        let mut result_stream = sqlx::query!(
                            r#"SELECT id as "id: CrateId", name FROM crates ORDER BY name"#
                        )
                        .fetch(&mut *list_conn);

                        while let Some(row) = result_stream.next().await {
                            let row = row?;

                            println!("handling crate {}", row.name);

                            db::update_latest_version_id(&mut update_conn, row.id).await?;
                        }

                        Ok::<(), anyhow::Error>(())
                    })
                    .context("Failed to update latest version id")?
            }

            Self::UpdateRepositoryFields => {
                ctx.runtime()?
                    .block_on(ctx.repository_stats_updater()?.update_all_crates())?;
            }

            Self::BackfillRepositoryStats => {
                ctx.runtime()?
                    .block_on(ctx.repository_stats_updater()?.backfill_repositories())?;
            }

            Self::UpdateCrateRegistryFields { name } => ctx.runtime()?.block_on(async move {
                let mut conn = ctx.pool()?.get_async().await?;
                let registry_data = ctx.registry_api()?.get_crate_data(&name).await?;
                db::update_crate_data_in_database(&mut conn, &name, &registry_data).await
            })?,

            Self::AddDirectory { directory } => {
                ctx.runtime()?
                    .block_on(async {
                        let storage = ctx.async_storage().await?;

                        add_path_into_database(&storage, &ctx.config()?.prefix, directory).await
                    })
                    .context("Failed to add directory into database")?;
            }

            Self::Delete {
                command: DeleteSubcommand::Version { name, version },
            } => ctx
                .runtime()?
                .block_on(async move {
                    let mut conn = ctx.pool()?.get_async().await?;
                    db::delete_version(
                        &mut conn,
                        &*ctx.async_storage().await?,
                        &*ctx.config()?,
                        &name,
                        &version,
                    )
                    .await
                })
                .context("failed to delete the version")?,
            Self::Delete {
                command: DeleteSubcommand::Crate { name },
            } => ctx
                .runtime()?
                .block_on(async move {
                    let mut conn = ctx.pool()?.get_async().await?;
                    db::delete_crate(
                        &mut conn,
                        &*ctx.async_storage().await?,
                        &*ctx.config()?,
                        &name,
                    )
                    .await
                })
                .context("failed to delete the crate")?,
            Self::Blacklist { command } => command.handle_args(ctx)?,

            Self::Limits { command } => command.handle_args(ctx)?,

            Self::Synchronize { dry_run } => {
                ctx.runtime()?
                    .block_on(docs_rs::utils::consistency::run_check(&ctx, dry_run))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum LimitsSubcommand {
    /// Get sandbox limit overrides for a crate
    Get { crate_name: String },

    /// List sandbox limit overrides for all crates
    List,

    /// Set sandbox limits overrides for a crate
    Set {
        crate_name: String,
        #[arg(long)]
        memory: Option<usize>,
        #[arg(long)]
        targets: Option<usize>,
        #[arg(long)]
        timeout: Option<Duration>,
    },

    /// Remove sandbox limits overrides for a crate
    Remove { crate_name: String },
}

impl LimitsSubcommand {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        let pool = ctx.pool()?;
        ctx.runtime()?.block_on(async move {
            let mut conn = pool.get_async().await?;

            match self {
                Self::Get { crate_name } => {
                    let overrides = Overrides::for_crate(&mut conn, &crate_name).await?;
                    println!("sandbox limit overrides for {crate_name} = {overrides:?}");
                }

                Self::List => {
                    for (crate_name, overrides) in Overrides::all(&mut conn).await? {
                        println!("sandbox limit overrides for {crate_name} = {overrides:?}");
                    }
                }

                Self::Set {
                    crate_name,
                    memory,
                    targets,
                    timeout,
                } => {
                    let overrides = Overrides::for_crate(&mut conn, &crate_name).await?;
                    println!("previous sandbox limit overrides for {crate_name} = {overrides:?}");
                    let overrides = Overrides {
                        memory,
                        targets,
                        timeout: timeout.map(Into::into),
                    };
                    Overrides::save(&mut conn, &crate_name, overrides).await?;
                    let overrides = Overrides::for_crate(&mut conn, &crate_name).await?;
                    println!("new sandbox limit overrides for {crate_name} = {overrides:?}");
                }

                Self::Remove { crate_name } => {
                    let overrides = Overrides::for_crate(&mut conn, &crate_name).await?;
                    println!("previous overrides for {crate_name} = {overrides:?}");
                    Overrides::remove(&mut conn, &crate_name).await?;
                }
            }
            Ok(())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum BlacklistSubcommand {
    /// List all crates on the blacklist
    List,

    /// Add a crate to the blacklist
    Add {
        /// Crate name
        #[arg(name = "CRATE_NAME")]
        crate_name: String,
    },

    /// Remove a crate from the blacklist
    Remove {
        /// Crate name
        #[arg(name = "CRATE_NAME")]
        crate_name: String,
    },
}

impl BlacklistSubcommand {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        ctx.runtime()?.block_on(async {
            let conn = &mut *ctx.pool()?.get_async().await?;
            match self {
                Self::List => {
                    let crates = db::blacklist::list_crates(conn)
                        .await
                        .context("failed to list crates on blacklist")?;

                    println!("{}", crates.join("\n"));
                }

                Self::Add { crate_name } => db::blacklist::add_crate(conn, &crate_name)
                    .await
                    .context("failed to add crate to blacklist")?,

                Self::Remove { crate_name } => db::blacklist::remove_crate(conn, &crate_name)
                    .await
                    .context("failed to remove crate from blacklist")?,
            }
            Ok(())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum DeleteSubcommand {
    /// Delete a whole crate
    Crate {
        /// Name of the crate to delete
        #[arg(name = "CRATE_NAME")]
        name: String,
    },
    /// Delete a single version of a crate (which may include multiple builds)
    Version {
        /// Name of the crate to delete
        #[arg(name = "CRATE_NAME")]
        name: String,

        /// The version of the crate to delete
        #[arg(name = "VERSION")]
        version: String,
    },
}

struct BinContext {
    build_queue: OnceCell<Arc<BuildQueue>>,
    async_build_queue: tokio::sync::OnceCell<Arc<AsyncBuildQueue>>,
    storage: OnceCell<Arc<Storage>>,
    cdn: tokio::sync::OnceCell<Arc<CdnBackend>>,
    config: OnceCell<Arc<Config>>,
    pool: OnceCell<Pool>,
    service_metrics: OnceCell<Arc<ServiceMetrics>>,
    instance_metrics: OnceCell<Arc<InstanceMetrics>>,
    index: OnceCell<Arc<Index>>,
    registry_api: OnceCell<Arc<RegistryApi>>,
    repository_stats_updater: OnceCell<Arc<RepositoryStatsUpdater>>,
    runtime: OnceCell<Arc<Runtime>>,
}

impl BinContext {
    fn new() -> Self {
        Self {
            build_queue: OnceCell::new(),
            async_build_queue: tokio::sync::OnceCell::new(),
            storage: OnceCell::new(),
            cdn: tokio::sync::OnceCell::new(),
            config: OnceCell::new(),
            pool: OnceCell::new(),
            service_metrics: OnceCell::new(),
            instance_metrics: OnceCell::new(),
            index: OnceCell::new(),
            registry_api: OnceCell::new(),
            repository_stats_updater: OnceCell::new(),
            runtime: OnceCell::new(),
        }
    }
}

macro_rules! lazy {
    ( $(fn $name:ident($self:ident) -> $type:ty = $init:expr);+ $(;)? ) => {
        $(fn $name(&$self) -> Result<Arc<$type>> {
            Ok($self
                .$name
                .get_or_try_init::<_, Error>(|| Ok(Arc::new($init)))?
                .clone())
        })*
    }
}

impl Context for BinContext {
    lazy! {
        fn build_queue(self) -> BuildQueue = {
            let runtime = self.runtime()?;
            BuildQueue::new(
                runtime.clone(),
                runtime.block_on(self.async_build_queue())?
            )
        };
        fn storage(self) -> Storage = {
            let runtime = self.runtime()?;
            Storage::new(
                runtime.block_on(self.async_storage())?,
                runtime
           )
        };
        fn config(self) -> Config = Config::from_env()?;
        fn service_metrics(self) -> ServiceMetrics = {
            ServiceMetrics::new()?
        };
        fn instance_metrics(self) -> InstanceMetrics = InstanceMetrics::new()?;
        fn runtime(self) -> Runtime = {
            Builder::new_multi_thread()
                .enable_all()
                .build()?
        };
        fn index(self) -> Index = {
            let config = self.config()?;
            let path = config.registry_index_path.clone();
            if let Some(registry_url) = config.registry_url.clone() {
                Index::from_url(path, registry_url)
            } else {
                Index::new(path)
            }?
        };
        fn registry_api(self) -> RegistryApi = {
            let config = self.config()?;
            RegistryApi::new(config.registry_api_host.clone(), config.crates_io_api_call_retries)?
        };
        fn repository_stats_updater(self) -> RepositoryStatsUpdater = {
            let config = self.config()?;
            let pool = self.pool()?;
            RepositoryStatsUpdater::new(&config, pool)
        };
    }

    async fn async_pool(&self) -> Result<Pool> {
        self.pool()
    }

    fn pool(&self) -> Result<Pool> {
        Ok(self
            .pool
            .get_or_try_init::<_, Error>(|| {
                Ok(Pool::new(
                    &*self.config()?,
                    self.runtime()?,
                    self.instance_metrics()?,
                )?)
            })?
            .clone())
    }

    async fn async_storage(&self) -> Result<Arc<AsyncStorage>> {
        Ok(Arc::new(
            AsyncStorage::new(self.pool()?, self.instance_metrics()?, self.config()?).await?,
        ))
    }

    async fn async_build_queue(&self) -> Result<Arc<AsyncBuildQueue>> {
        Ok(self
            .async_build_queue
            .get_or_try_init(|| async {
                Ok::<_, Error>(Arc::new(AsyncBuildQueue::new(
                    self.pool()?,
                    self.instance_metrics()?,
                    self.config()?,
                    self.async_storage().await?,
                )))
            })
            .await?
            .clone())
    }

    async fn cdn(&self) -> Result<Arc<CdnBackend>> {
        let config = self.config()?;
        Ok(self
            .cdn
            .get_or_init(|| async { Arc::new(CdnBackend::new(&config).await) })
            .await
            .clone())
    }
}
