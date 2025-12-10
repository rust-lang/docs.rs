use anyhow::{Context as _, Result, anyhow};
use chrono::NaiveDate;
use clap::{Parser, Subcommand, ValueEnum};
use docs_rs::{
    Config, Context, PackageKind, RustwideBuilder,
    db::{self, Overrides},
    start_web_server,
    utils::queue_builder,
};
use docs_rs_build_queue::rebuilds::queue_rebuilds_faulty_rustdoc;
use docs_rs_database::{
    service_config::{ConfigName, set_config},
    types::{CrateId, version::Version},
};
use futures_util::StreamExt;
use std::{env, fmt::Write, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::runtime;
use tracing_log::LogTracer;

fn main() {
    // set the global log::logger for backwards compatibility
    // through rustwide.
    let _logging_guard = rustwide::logging::init_with(LogTracer::new());
    docs_rs_logging::init();

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
        drop(_logging_guard);
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
    version = docs_rs_utils::BUILD_VERSION,
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
        let config = Config::from_env()?;
        let runtime = Arc::new(runtime::Builder::new_multi_thread().enable_all().build()?);
        let ctx = runtime.block_on(Context::from_config(config))?;

        match self {
            Self::Build { subcommand } => subcommand.handle_args(ctx)?,
            Self::StartBuildServer => {
                queue_builder(&ctx, RustwideBuilder::init(&ctx)?)?;
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
        crate_version: Version,
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

    /// Queue rebuilds for broken nightly versions of rustdoc, either for a single date (start) or a range (start inclusive, end exclusive)
    RebuildBrokenNightly {
        /// Start date of nightly builds to rebuild (inclusive)
        #[arg(name = "START", short = 's', long = "start")]
        start_nightly_date: NaiveDate,

        /// End date of nightly builds to rebuild (exclusive, optional)
        #[arg(name = "END", short = 'e', long = "end")]
        end_nightly_date: Option<NaiveDate>,
    },
}

impl QueueSubcommand {
    fn handle_args(self, ctx: Context) -> Result<()> {
        match self {
            Self::Add {
                crate_name,
                crate_version,
                build_priority,
            } => ctx.build_queue.add_crate(
                &crate_name,
                &crate_version,
                build_priority,
                ctx.config.watcher.registry_url.as_deref(),
            )?,


            Self::RebuildBrokenNightly { start_nightly_date, end_nightly_date } => {
                ctx.runtime.block_on(async move {
                    let mut conn = ctx.pool.get_async().await?;
                    let queued_rebuilds_amount = queue_rebuilds_faulty_rustdoc(&mut conn, &ctx.async_build_queue, &start_nightly_date, &end_nightly_date).await?;
                    println!("Queued {queued_rebuilds_amount} rebuilds for broken nightly versions of rustdoc");
                    Ok::<(), anyhow::Error>(())
                })?
            }
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

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum BuildSubcommand {
    /// Builds documentation for a crate
    Crate {
        /// Crate name
        #[arg(name = "CRATE_NAME", requires("CRATE_VERSION"))]
        crate_name: Option<String>,

        /// Version of crate
        #[arg(name = "CRATE_VERSION")]
        crate_version: Option<Version>,

        /// Build a crate at a specific path
        #[arg(short = 'l', long = "local", conflicts_with_all(&["CRATE_NAME", "CRATE_VERSION"]))]
        local: Option<PathBuf>,
    },

    SetToolchain {
        toolchain_name: String,
    },

    /// Locks the daemon, preventing it from building new crates
    Lock,

    /// Unlocks the daemon to continue building new crates
    Unlock,
}

impl BuildSubcommand {
    fn handle_args(self, ctx: Context) -> Result<()> {
        let rustwide_builder = || -> Result<RustwideBuilder> { RustwideBuilder::init(&ctx) };

        match self {
            Self::Crate {
                crate_name,
                crate_version,
                local,
            } => {
                let mut builder = rustwide_builder()?;

                builder.update_toolchain_and_add_essential_files()?;

                if let Some(path) = local {
                    builder
                        .build_local_package(&path)
                        .context("Building documentation failed")?;
                } else {
                    let registry_url = ctx.config.watcher.registry_url.as_ref();
                    builder
                        .build_package(
                            &crate_name
                                .with_context(|| anyhow!("must specify name if not local"))?,
                            &crate_version
                                .with_context(|| anyhow!("must specify version if not local"))?,
                            registry_url
                                .map(|s| PackageKind::Registry(s.as_str()))
                                .unwrap_or(PackageKind::CratesIo),
                            true,
                        )
                        .context("Building documentation failed")?;
                }
            }

            Self::SetToolchain { toolchain_name } => {
                ctx.runtime.block_on(async move {
                    let mut conn = ctx
                        .pool
                        .get_async()
                        .await
                        .context("failed to get a database connection")?;
                    set_config(&mut conn, ConfigName::Toolchain, toolchain_name)
                        .await
                        .context("failed to set toolchain in database")
                })?;
            }

            Self::Lock => ctx.build_queue.lock().context("Failed to lock")?,
            Self::Unlock => ctx.build_queue.unlock().context("Failed to unlock")?,
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

    /// Limit overrides operations
    Limits {
        #[command(subcommand)]
        command: LimitsSubcommand,
    },
}

impl DatabaseSubcommand {
    fn handle_args(self, ctx: Context) -> Result<()> {
        match self {
            Self::Migrate { version } => ctx
                .runtime
                .block_on(async {
                    let mut conn = ctx.pool.get_async().await?;
                    db::migrate(&mut conn, version).await
                })
                .context("Failed to run database migrations")?,

            Self::UpdateLatestVersionId => ctx
                .runtime
                .block_on(async {
                    let mut list_conn = ctx.pool.get_async().await?;
                    let mut update_conn = ctx.pool.get_async().await?;

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
                .context("Failed to update latest version id")?,

            Self::UpdateRepositoryFields => {
                ctx.runtime
                    .block_on(ctx.repository_stats_updater.update_all_crates())?;
            }

            Self::BackfillRepositoryStats => {
                ctx.runtime
                    .block_on(ctx.repository_stats_updater.backfill_repositories())?;
            }

            Self::UpdateCrateRegistryFields { name } => ctx.runtime.block_on(async move {
                let mut conn = ctx.pool.get_async().await?;
                let registry_data = ctx.registry_api.get_crate_data(&name).await?;
                db::update_crate_data_in_database(&mut conn, &name, &registry_data).await
            })?,

            Self::Limits { command } => command.handle_args(ctx)?,
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
        timeout: Option<usize>,
    },

    /// Remove sandbox limits overrides for a crate
    Remove { crate_name: String },
}

impl LimitsSubcommand {
    fn handle_args(self, ctx: Context) -> Result<()> {
        ctx.runtime.block_on(async move {
            let mut conn = ctx.pool.get_async().await?;

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
                        timeout: timeout
                            .map(|timeout| std::time::Duration::from_secs(timeout as _)),
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
    fn handle_args(self, ctx: Context) -> Result<()> {
        ctx.runtime.block_on(async move {
            let conn = &mut ctx.pool.get_async().await?;
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
