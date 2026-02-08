mod rebuilds;
mod repackage;
#[cfg(test)]
pub(crate) mod testing;

use anyhow::{Context as _, Result, bail};
use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use docs_rs_build_limits::{Overrides, blacklist};
use docs_rs_build_queue::priority::{
    get_crate_pattern_and_priority, list_crate_priorities, remove_crate_priority,
    set_crate_priority,
};
use docs_rs_context::Context;
use docs_rs_database::{
    crate_details,
    service_config::{ConfigName, set_config},
};
use docs_rs_fastly::CdnBehaviour as _;
use docs_rs_headers::SurrogateKey;
use docs_rs_repository_stats::workspaces;
use docs_rs_types::{CrateId, KrateName, ReleaseId, Version};
use futures_util::StreamExt;
use rebuilds::queue_rebuilds_faulty_rustdoc;
use std::iter;

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = docs_rs_logging::init().context("error initializing logging")?;

    if let Err(err) = CommandLine::parse().handle_args().await {
        eprintln!("error running admin CLI: {:?}", err);
        drop(_guard);
        std::process::exit(1);
    }

    Ok(())
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

    Cdn {
        #[command(subcommand)]
        subcommand: CdnSubcommand,
    },
}

impl CommandLine {
    async fn handle_args(self) -> Result<()> {
        let ctx = Context::builder()
            .with_runtime()
            .await?
            .with_meter_provider()?
            .with_pool()
            .await?
            .with_storage()
            .await?
            .with_build_queue()?
            .with_repository_stats()?
            .with_registry_api()?
            .with_maybe_cdn()?
            .build()?;

        match self {
            Self::Build { subcommand } => subcommand.handle_args(ctx).await?,
            Self::Database { subcommand } => subcommand.handle_args(ctx).await?,
            Self::Queue { subcommand } => subcommand.handle_args(ctx).await?,
            Self::Cdn { subcommand } => subcommand.handle_args(ctx).await?,
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
        crate_name: KrateName,
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
    async fn handle_args(self, ctx: Context) -> Result<()> {
        match self {
            Self::Add {
                crate_name,
                crate_version,
                build_priority,
            } => {
                ctx.build_queue()?
                    .add_crate(&crate_name, &crate_version, build_priority, None)
                    .await?
            }

            Self::DefaultPriority { subcommand } => subcommand.handle_args(ctx).await?,

            Self::RebuildBrokenNightly {
                start_nightly_date,
                end_nightly_date,
            } => {
                let mut conn = ctx.pool()?.get_async().await?;
                let queued_rebuilds_amount = queue_rebuilds_faulty_rustdoc(
                    &mut conn,
                    ctx.build_queue()?,
                    &start_nightly_date,
                    &end_nightly_date,
                )
                .await?;
                println!(
                    "Queued {queued_rebuilds_amount} rebuilds for broken nightly versions of rustdoc"
                );
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
    Get { crate_name: KrateName },

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
    async fn handle_args(self, ctx: Context) -> Result<()> {
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
    async fn handle_args(self, ctx: Context) -> Result<()> {
        match self {
            Self::SetToolchain { toolchain_name } => {
                let mut conn = ctx
                    .pool()?
                    .get_async()
                    .await
                    .context("failed to get a database connection")?;
                set_config(&mut conn, ConfigName::Toolchain, toolchain_name)
                    .await
                    .context("failed to set toolchain in database")?;
            }

            Self::Lock => ctx.build_queue()?.lock().await.context("Failed to lock")?,
            Self::Unlock => ctx
                .build_queue()?
                .unlock()
                .await
                .context("Failed to unlock")?,
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

    /// temporary command to repackage missing crates into archive storage.
    /// starts at the earliest release and works forwards.
    Repackage {
        /// process at most this amount of releases
        #[arg(long)]
        limit: Option<u32>,
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
        name: KrateName,
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
    async fn handle_args(self, ctx: Context) -> Result<()> {
        match self {
            Self::Migrate { version } => {
                let mut conn = ctx.pool()?.get_async().await?;
                docs_rs_database::migrate(&mut conn, version).await
            }
            .context("Failed to run database migrations")?,

            Self::Repackage { limit } => {
                let pool = ctx.pool()?;
                let storage = ctx.storage()?;
                let mut list_conn = pool.get_async().await?;
                let mut update_conn = pool.get_async().await?;

                let limit = limit.unwrap_or(2_000_000u32);

                let mut stream = sqlx::query!(
                    r#"SELECT
                           r.id as "rid: ReleaseId",
                           c.name as "name: KrateName",
                           r.version as "version: Version"
                       FROM
                            crates as c
                            INNER JOIN releases as r ON c.id = r.crate_id
                       WHERE
                            r.archive_storage = FALSE
                       ORDER BY r.id
                       LIMIT $1
                    "#,
                    limit as i64,
                )
                .fetch(&mut *list_conn);

                while let Some(row) = stream.next().await {
                    let row = row?;

                    crate::repackage::repackage(
                        &mut update_conn,
                        storage,
                        row.rid,
                        &row.name,
                        &row.version,
                    )
                    .await?;
                }

                Ok::<(), anyhow::Error>(())
            }
            .context("Failed to repackage storage")?,

            Self::UpdateLatestVersionId => {
                let pool = ctx.pool()?;
                let mut list_conn = pool.get_async().await?;
                let mut update_conn = pool.get_async().await?;

                let mut result_stream =
                    sqlx::query!(r#"SELECT id as "id: CrateId", name FROM crates ORDER BY name"#)
                        .fetch(&mut *list_conn);

                while let Some(row) = result_stream.next().await {
                    let row = row?;

                    println!("handling crate {}", row.name);

                    crate_details::update_latest_version_id(&mut update_conn, row.id).await?;
                }

                Ok::<(), anyhow::Error>(())
            }
            .context("Failed to update latest version id")?,

            Self::UpdateRepositoryFields => {
                println!("rewrite crate count per repository...");
                let mut conn = ctx.pool()?.get_async().await?;
                workspaces::rewrite_repository_stats(&mut conn).await?;

                println!("update repository stats where outdated...");
                ctx.repository_stats()?.update_all_crates().await?;
            }

            Self::BackfillRepositoryStats => {
                println!("backfill repositories...");
                ctx.repository_stats()?.backfill_repositories().await?;

                println!("rewrite crate count per repository...");
                let mut conn = ctx.pool()?.get_async().await?;
                workspaces::rewrite_repository_stats(&mut conn).await?;
            }

            Self::UpdateCrateRegistryFields { name } => {
                let mut conn = ctx.pool()?.get_async().await?;
                let registry_data = ctx.registry_api()?.get_crate_data(&name).await?;
                docs_rs_database::releases::update_crate_data_in_database(
                    &mut conn,
                    &name,
                    &registry_data,
                )
                .await?;
            }

            Self::Blacklist { command } => command.handle_args(ctx).await?,

            Self::Limits { command } => command.handle_args(ctx).await?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum LimitsSubcommand {
    /// Get sandbox limit overrides for a crate
    Get { crate_name: KrateName },

    /// List sandbox limit overrides for all crates
    List,

    /// Set sandbox limits overrides for a crate
    Set {
        crate_name: KrateName,
        #[arg(long)]
        memory: Option<usize>,
        #[arg(long)]
        targets: Option<usize>,
        #[arg(long)]
        timeout: Option<usize>,
    },

    /// Remove sandbox limits overrides for a crate
    Remove { crate_name: KrateName },
}

impl LimitsSubcommand {
    async fn handle_args(self, ctx: Context) -> Result<()> {
        let mut conn = ctx.pool()?.get_async().await?;

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
                    timeout: timeout.map(|timeout| std::time::Duration::from_secs(timeout as _)),
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
    async fn handle_args(self, ctx: Context) -> Result<()> {
        let mut conn = ctx.pool()?.get_async().await?;
        match self {
            Self::List => {
                let crates: Vec<_> = blacklist::list_crates(&mut conn)
                    .await
                    .context("failed to list crates on blacklist")?
                    .into_iter()
                    .map(|k| k.to_string())
                    .collect();

                println!("{}", crates.join("\n"));
            }

            Self::Add { crate_name } => blacklist::add_crate(&mut conn, &crate_name)
                .await
                .context("failed to add crate to blacklist")?,

            Self::Remove { crate_name } => blacklist::remove_crate(&mut conn, &crate_name)
                .await
                .context("failed to remove crate from blacklist")?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum CdnSubcommand {
    /// purge pages with a surrogate key from the CDN
    Purge {
        /// Name of crate to build
        #[arg(name = "SURROGATE_KEY")]
        surrogate_key: SurrogateKey,
    },
}

impl CdnSubcommand {
    async fn handle_args(self, ctx: Context) -> Result<()> {
        match self {
            Self::Purge { surrogate_key } => {
                if let Some(cdn) = ctx.cdn() {
                    cdn.purge_surrogate_keys(iter::once(surrogate_key))
                        .await
                        .context("failed to purge CDN by surrogate key")?;
                } else {
                    bail!("CDN is not configured, cannot purge");
                }
            }
        }
        Ok(())
    }
}
