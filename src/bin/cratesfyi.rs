use std::env;
use std::fmt::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Error, Result};
use clap::{Parser, Subcommand, ValueEnum};
use docs_rs::cdn::CdnBackend;
use docs_rs::db::{self, add_path_into_database, Overrides, Pool, PoolClient};
use docs_rs::repositories::RepositoryStatsUpdater;
use docs_rs::utils::{
    get_config, get_crate_pattern_and_priority, list_crate_priorities, queue_builder,
    remove_crate_priority, set_crate_priority, ConfigName,
};
use docs_rs::{
    start_web_server, BuildQueue, Config, Context, Index, Metrics, PackageKind, RustwideBuilder,
    Storage,
};
use humantime::Duration;
use once_cell::sync::OnceCell;
use tokio::runtime::{Builder, Runtime};
use tracing_log::LogTracer;
use tracing_subscriber::{filter::Directive, prelude::*, EnvFilter};

fn main() {
    // set the global log::logger for backwards compatibility
    // through rustwide.
    rustwide::logging::init_with(LogTracer::new());

    let tracing_registry = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(Directive::from_str("docs_rs=info").unwrap())
                .with_env_var("DOCSRS_LOG")
                .from_env_lossy(),
        );

    let _sentry_guard = if let Ok(sentry_dsn) = env::var("SENTRY_DSN") {
        tracing_registry
            .with(sentry_tracing::layer().event_filter(|md| {
                if md.fields().field("reported_to_sentry").is_some() {
                    sentry_tracing::EventFilter::Ignore
                } else {
                    sentry_tracing::default_event_filter(md)
                }
            }))
            .init();
        Some(sentry::init((
            sentry_dsn,
            sentry::ClientOptions {
                release: Some(docs_rs::BUILD_VERSION.into()),
                attach_stacktrace: true,
                ..Default::default()
            }
            .add_integration(sentry_panic::PanicIntegration::default()),
        )))
    } else {
        tracing_registry.init();
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
        /// Skips building documentation if documentation exists
        #[arg(name = "SKIP_IF_EXISTS", short = 's', long = "skip")]
        skip_if_exists: bool,

        #[command(subcommand)]
        subcommand: BuildSubcommand,
    },

    /// Starts web server
    StartWebServer {
        #[arg(name = "SOCKET_ADDR", default_value = "0.0.0.0:3000")]
        socket_addr: String,
    },

    StartRegistryWatcher {
        /// Enable or disable the repository stats updater
        #[arg(
            long = "repository-stats-updater",
            default_value = "disabled",
            value_enum
        )]
        repository_stats_updater: Toggle,
        #[arg(long = "cdn-invalidator", default_value = "enabled", value_enum)]
        cdn_invalidator: Toggle,
    },

    StartBuildServer,

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
            Self::Build {
                skip_if_exists,
                subcommand,
            } => subcommand.handle_args(ctx, skip_if_exists)?,
            Self::StartRegistryWatcher {
                repository_stats_updater,
                cdn_invalidator,
            } => {
                if repository_stats_updater == Toggle::Enabled {
                    docs_rs::utils::daemon::start_background_repository_stats_updater(&ctx)?;
                }
                if cdn_invalidator == Toggle::Enabled {
                    docs_rs::utils::daemon::start_background_cdn_invalidator(&ctx)?;
                }

                docs_rs::utils::watch_registry(ctx.build_queue()?, ctx.config()?, ctx.index()?)?;
            }
            Self::StartBuildServer => {
                let build_queue = ctx.build_queue()?;
                let rustwide_builder = RustwideBuilder::init(&ctx)?;
                queue_builder(rustwide_builder, build_queue)?;
            }
            Self::StartWebServer { socket_addr } => {
                // Blocks indefinitely
                start_web_server(Some(&socket_addr), &ctx)?;
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

    /// Set the register watcher's last seen reference
    SetLastSeenReference {
        reference: crates_index_diff::gix::ObjectId,
    },
}

impl QueueSubcommand {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        match self {
            Self::Add {
                crate_name,
                crate_version,
                build_priority,
            } => ctx.build_queue()?.add_crate(
                &crate_name,
                &crate_version,
                build_priority,
                ctx.config()?.registry_url.as_deref(),
            )?,

            Self::GetLastSeenReference => {
                if let Some(reference) = ctx.build_queue()?.last_seen_reference()? {
                    println!("Last seen reference: {reference}");
                } else {
                    println!("No last seen reference available");
                }
            }

            Self::SetLastSeenReference { reference } => {
                ctx.build_queue()?.set_last_seen_reference(reference)?;
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
        let conn = &mut *ctx.conn()?;
        match self {
            Self::List => {
                for (pattern, priority) in list_crate_priorities(conn)? {
                    println!("{pattern:>20} : {priority:>3}");
                }
            }

            Self::Get { crate_name } => {
                if let Some((pattern, priority)) =
                    get_crate_pattern_and_priority(conn, &crate_name)?
                {
                    println!("{pattern} : {priority}");
                } else {
                    println!("No priority found for {crate_name}");
                }
            }

            Self::Set { pattern, priority } => {
                set_crate_priority(conn, &pattern, priority)
                    .context("Could not set pattern's priority")?;
                println!("Set pattern '{pattern}' to priority {priority}");
            }

            Self::Remove { pattern } => {
                if let Some(priority) = remove_crate_priority(conn, &pattern)
                    .context("Could not remove pattern's priority")?
                {
                    println!("Removed pattern '{pattern}' with priority {priority}");
                } else {
                    println!("Pattern '{pattern}' did not exist and so was not removed");
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum BuildSubcommand {
    /// Builds documentation of every crate
    World,

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

    /// Locks the daemon, preventing it from building new crates
    Lock,

    /// Unlocks the daemon to continue building new crates
    Unlock,
}

impl BuildSubcommand {
    fn handle_args(self, ctx: BinContext, skip_if_exists: bool) -> Result<()> {
        let build_queue = ctx.build_queue()?;

        let rustwide_builder = || -> Result<RustwideBuilder> {
            let mut builder = RustwideBuilder::init(&ctx)?;
            builder.set_skip_build_if_exists(skip_if_exists);
            Ok(builder)
        };

        match self {
            Self::World => {
                rustwide_builder()?
                    .build_world()
                    .context("Failed to build world")?;
            }

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
                if only_first_time {
                    let mut conn = ctx
                        .pool()?
                        .get()
                        .context("failed to get a database connection")?;

                    if get_config::<String>(&mut conn, ConfigName::RustcVersion)?.is_some() {
                        println!("update-toolchain was already called in the past, exiting");
                        return Ok(());
                    }
                }

                rustwide_builder()?
                    .update_toolchain()
                    .context("failed to update toolchain")?;
            }

            Self::AddEssentialFiles => {
                rustwide_builder()?
                    .add_essential_files()
                    .context("failed to add essential files")?;
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

    /// Updates Github/Gitlab stats for crates.
    UpdateRepositoryFields,

    /// Backfill GitHub/Gitlab stats for crates.
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
    #[cfg(feature = "consistency_check")]
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
                db::migrate(version, &mut *ctx.conn()?)
                    .context("Failed to run database migrations")?;
            }

            Self::UpdateRepositoryFields => {
                ctx.repository_stats_updater()?.update_all_crates()?;
            }

            Self::BackfillRepositoryStats => {
                ctx.repository_stats_updater()?.backfill_repositories()?;
            }

            Self::UpdateCrateRegistryFields { name } => {
                let index = ctx.index()?;

                db::update_crate_data_in_database(
                    &mut *ctx.conn()?,
                    &name,
                    &index.api().get_crate_data(&name)?,
                )?;
            }

            Self::AddDirectory { directory } => {
                add_path_into_database(&*ctx.storage()?, &ctx.config()?.prefix, directory)
                    .context("Failed to add directory into database")?;
            }

            Self::Delete {
                command: DeleteSubcommand::Version { name, version },
            } => {
                db::delete_version(&ctx, &name, &version).context("failed to delete the version")?
            }
            Self::Delete {
                command: DeleteSubcommand::Crate { name },
            } => db::delete_crate(
                &mut *ctx.pool()?.get()?,
                &*ctx.storage()?,
                &*ctx.config()?,
                &name,
            )
            .context("failed to delete the crate")?,
            Self::Blacklist { command } => command.handle_args(ctx)?,

            Self::Limits { command } => command.handle_args(ctx)?,

            #[cfg(feature = "consistency_check")]
            Self::Synchronize { dry_run } => {
                docs_rs::utils::consistency::run_check(&ctx, dry_run)?;
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
        let conn = &mut *ctx.conn()?;
        match self {
            Self::Get { crate_name } => {
                let overrides = Overrides::for_crate(conn, &crate_name)?;
                println!("sandbox limit overrides for {crate_name} = {overrides:?}");
            }

            Self::List => {
                for (crate_name, overrides) in Overrides::all(conn)? {
                    println!("sandbox limit overrides for {crate_name} = {overrides:?}");
                }
            }

            Self::Set {
                crate_name,
                memory,
                targets,
                timeout,
            } => {
                let overrides = Overrides::for_crate(conn, &crate_name)?;
                println!("previous sandbox limit overrides for {crate_name} = {overrides:?}");
                let overrides = Overrides {
                    memory,
                    targets,
                    timeout: timeout.map(Into::into),
                };
                Overrides::save(conn, &crate_name, overrides)?;
                let overrides = Overrides::for_crate(conn, &crate_name)?;
                println!("new sandbox limit overrides for {crate_name} = {overrides:?}");
            }

            Self::Remove { crate_name } => {
                let overrides = Overrides::for_crate(conn, &crate_name)?;
                println!("previous overrides for {crate_name} = {overrides:?}");
                Overrides::remove(conn, &crate_name)?;
            }
        }
        Ok(())
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
        let conn = &mut *ctx.conn()?;
        match self {
            Self::List => {
                let crates = db::blacklist::list_crates(conn)
                    .context("failed to list crates on blacklist")?;

                println!("{}", crates.join("\n"));
            }

            Self::Add { crate_name } => db::blacklist::add_crate(conn, &crate_name)
                .context("failed to add crate to blacklist")?,

            Self::Remove { crate_name } => db::blacklist::remove_crate(conn, &crate_name)
                .context("failed to remove crate from blacklist")?,
        }
        Ok(())
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
    storage: OnceCell<Arc<Storage>>,
    cdn: OnceCell<Arc<CdnBackend>>,
    config: OnceCell<Arc<Config>>,
    pool: OnceCell<Pool>,
    metrics: OnceCell<Arc<Metrics>>,
    index: OnceCell<Arc<Index>>,
    repository_stats_updater: OnceCell<Arc<RepositoryStatsUpdater>>,
    runtime: OnceCell<Arc<Runtime>>,
}

impl BinContext {
    fn new() -> Self {
        Self {
            build_queue: OnceCell::new(),
            storage: OnceCell::new(),
            cdn: OnceCell::new(),
            config: OnceCell::new(),
            pool: OnceCell::new(),
            metrics: OnceCell::new(),
            index: OnceCell::new(),
            repository_stats_updater: OnceCell::new(),
            runtime: OnceCell::new(),
        }
    }

    fn conn(&self) -> Result<PoolClient> {
        Ok(self.pool()?.get()?)
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
        fn build_queue(self) -> BuildQueue = BuildQueue::new(
            self.pool()?,
            self.metrics()?,
            self.config()?,
            self.storage()?,
        );
        fn storage(self) -> Storage = Storage::new(
            self.pool()?,
            self.metrics()?,
            self.config()?,
            self.runtime()?,
        )?;
        fn cdn(self) -> CdnBackend = CdnBackend::new(
            &self.config()?,
            &self.runtime()?,
        );
        fn config(self) -> Config = Config::from_env()?;
        fn metrics(self) -> Metrics = Metrics::new()?;
        fn runtime(self) -> Runtime = {
            Builder::new_multi_thread()
                .enable_all()
                .build()?
        };
        fn index(self) -> Index = {
            let config = self.config()?;
            let path = config.registry_index_path.clone();
            if let Some(registry_url) = config.registry_url.clone() {
                Index::from_url(path, registry_url, config.crates_io_api_call_retries)
            } else {
                Index::new(path, config.crates_io_api_call_retries)
            }?
        };
        fn repository_stats_updater(self) -> RepositoryStatsUpdater = {
            let config = self.config()?;
            let pool = self.pool()?;
            RepositoryStatsUpdater::new(&config, pool)
        };
    }

    fn pool(&self) -> Result<Pool> {
        Ok(self
            .pool
            .get_or_try_init::<_, Error>(|| Ok(Pool::new(&*self.config()?, self.metrics()?)?))?
            .clone())
    }
}
