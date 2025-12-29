use anyhow::{Context as _, Result};
use chrono::NaiveDate;
use clap::{Parser, Subcommand, ValueEnum};
use docs_rs::{Config, Context, start_web_server};
use docs_rs_build_limits::Overrides;
use docs_rs_build_queue::priority::{
    get_crate_pattern_and_priority, list_crate_priorities, remove_crate_priority,
    set_crate_priority,
};
use docs_rs_builder::{RustwideBuilder, blacklist, queue_builder};
use docs_rs_context::Context as NewContext;
use docs_rs_database::{
    crate_details,
    service_config::{ConfigName, set_config},
};
use docs_rs_storage::add_path_into_database;
use docs_rs_types::{CrateId, KrateName, Version};
use docs_rs_watcher::{queue_rebuilds_faulty_rustdoc, start_background_service_metric_collector};
use futures_util::StreamExt;
use std::{env, fmt::Write, net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::runtime;

fn main() {
    // set the global log::logger for backwards compatibility
    // through rustwide.
    docs_rs_builder::logging::init();
    let guard = docs_rs_logging::init().expect("error initializing logging");

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
        drop(guard);
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
        /// Enable or disable the repository stats updater
        #[arg(
            long = "repository-stats-updater",
            default_value = "disabled",
            value_enum
        )]
        repository_stats_updater: Toggle,
        #[arg(long = "queue-rebuilds", default_value = "enabled", value_enum)]
        queue_rebuilds: Toggle,
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
        let config = Config::from_env()?.build()?;
        let runtime = Arc::new(runtime::Builder::new_multi_thread().enable_all().build()?);
        let ctx = runtime.block_on(Context::from_config(config))?;
        let new_context = Arc::new(NewContext::from(&ctx));

        match self {
            Self::Build { subcommand } => subcommand.handle_args(ctx)?,
            Self::StartRegistryWatcher {
                repository_stats_updater,
                queue_rebuilds,
            } => ctx.runtime.block_on(async move {
                if repository_stats_updater == Toggle::Enabled {
                    docs_rs_watcher::start_background_repository_stats_updater(&new_context)
                        .await?;
                }
                if queue_rebuilds == Toggle::Enabled {
                    docs_rs_watcher::start_background_queue_rebuild(
                        ctx.config.watcher.clone(),
                        &new_context,
                    )
                    .await?;
                }

                // When people run the services separately, we assume that we can collect service
                // metrics from the registry watcher, which should only run once, and all the time.
                start_background_service_metric_collector(&new_context).await?;

                docs_rs_watcher::watch_registry(&ctx.config.watcher.clone(), &new_context).await
            })?,
            Self::StartBuildServer => {
                let builder_config = ctx.config.builder.clone();
                queue_builder(
                    &new_context,
                    &builder_config,
                    RustwideBuilder::init(builder_config.clone(), &new_context)?,
                )?;
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

    /// Interactions with build queue priorities
    DefaultPriority {
        #[command(subcommand)]
        subcommand: PrioritySubcommand,
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
            } => {
                let crate_name: KrateName = crate_name.parse()?;
                ctx.build_queue.add_crate(
                &crate_name,
                &crate_version,
                build_priority,
                ctx.config.watcher.registry_url.as_deref(),
            )?},


            Self::DefaultPriority { subcommand } => subcommand.handle_args(ctx)?,

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

impl PrioritySubcommand {
    fn handle_args(self, ctx: Context) -> Result<()> {
        ctx.runtime.block_on(async move {
            let mut conn = ctx.pool.get_async().await?;
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
        match self {
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

    AddDirectory {
        /// Path of file or directory
        #[arg(name = "DIRECTORY")]
        directory: PathBuf,
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
}

impl DatabaseSubcommand {
    fn handle_args(self, ctx: Context) -> Result<()> {
        match self {
            Self::Migrate { version } => ctx
                .runtime
                .block_on(async {
                    let mut conn = ctx.pool.get_async().await?;
                    docs_rs_database::migrate(&mut conn, version).await
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

                        crate_details::update_latest_version_id(&mut update_conn, row.id).await?;
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
                docs_rs_database::releases::update_crate_data_in_database(
                    &mut conn,
                    &name,
                    &registry_data,
                )
                .await
            })?,

            Self::AddDirectory { directory } => {
                ctx.runtime
                    .block_on(add_path_into_database(
                        &ctx.async_storage,
                        &ctx.config.prefix,
                        directory,
                    ))
                    .context("Failed to add directory into database")?;
            }

            Self::Blacklist { command } => command.handle_args(ctx)?,

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
                    let crate_name: KrateName = crate_name.parse()?;
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
                    let crate_name: KrateName = crate_name.parse()?;
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
                    let crate_name: KrateName = crate_name.parse()?;
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
        crate_name: KrateName,
    },

    /// Remove a crate from the blacklist
    Remove {
        /// Crate name
        #[arg(name = "CRATE_NAME")]
        crate_name: KrateName,
    },
}

impl BlacklistSubcommand {
    fn handle_args(self, ctx: Context) -> Result<()> {
        ctx.runtime.block_on(async move {
            let conn = &mut ctx.pool.get_async().await?;
            match self {
                Self::List => {
                    let crates: Vec<_> = blacklist::list_crates(conn)
                        .await
                        .context("failed to list crates on blacklist")?
                        .into_iter()
                        .map(|k| k.to_string())
                        .collect();

                    println!("{}", crates.join("\n"));
                }

                Self::Add { crate_name } => blacklist::add_crate(conn, &crate_name)
                    .await
                    .context("failed to add crate to blacklist")?,

                Self::Remove { crate_name } => blacklist::remove_crate(conn, &crate_name)
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
        version: Version,
    },
}
