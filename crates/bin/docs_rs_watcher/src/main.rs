use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use docs_rs_config::AppConfig as _;
use docs_rs_context::Context;
use docs_rs_types::{KrateName, Version};
use docs_rs_watcher::{Config, Index, index_watcher};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = docs_rs_logging::init().context("error initializing logging")?;

    if let Err(err) = CommandLine::parse().handle_args().await {
        eprintln!("error running watcher: {:?}", err);
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
    /// Run a regsitry euild-server
    Start {
        /// Enable or disable the repository stats updater
        #[arg(long = "repository-stats-updater", default_value = "true")]
        repository_stats_updater: bool,
        /// enable or disable the automatic rebuild of old releases
        #[arg(long = "queue-rebuilds", default_value = "true")]
        queue_rebuilds: bool,
    },

    /// Interactions with the build queue
    Queue {
        #[command(subcommand)]
        subcommand: QueueSubcommand,
    },

    /// Database operations
    Database {
        #[command(subcommand)]
        subcommand: DatabaseSubcommand,
    },
}

impl CommandLine {
    async fn handle_args(self) -> Result<()> {
        let config = Arc::new(Config::from_environment()?);
        let ctx = Context::builder()
            .with_runtime()
            .await?
            .with_meter_provider()?
            .with_pool()
            .await?
            .with_storage()
            .await?
            .with_maybe_cdn()?
            .with_build_queue()?
            .with_repository_stats()?
            .build()?;

        match self {
            Self::Start {
                repository_stats_updater,
                queue_rebuilds,
            } => {
                if repository_stats_updater {
                    docs_rs_watcher::start_background_repository_stats_updater(&ctx).await?;
                }
                if queue_rebuilds {
                    docs_rs_watcher::start_background_queue_rebuild(config.clone(), &ctx).await?;
                }

                // We assume that we can collect service metrics from the registry watcher,
                // which should only run once, and all the time.
                docs_rs_watcher::start_background_service_metric_collector(&ctx).await?;

                docs_rs_watcher::watch_registry(&config, &ctx).await?;
            }
            Self::Queue { subcommand } => subcommand.handle_args(config, ctx).await?,
            Self::Database { subcommand } => subcommand.handle_args(config, ctx).await?,
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum QueueSubcommand {
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
    async fn handle_args(self, config: Arc<Config>, ctx: Context) -> Result<()> {
        match self {
            Self::GetLastSeenReference => {
                let mut conn = ctx.pool()?.get_async().await?;
                if let Some(reference) = index_watcher::last_seen_reference(&mut conn).await? {
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
                        let index = Index::from_config(&config).await?;
                        index.latest_commit_reference().await?
                    }
                    (_, _) => unreachable!(),
                };

                let mut conn = ctx.pool()?.get_async().await?;

                index_watcher::set_last_seen_reference(&mut conn, reference).await?;
                println!("Set last seen reference: {reference}");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum DatabaseSubcommand {
    /// Updates GitHub/GitLab stats for crates.
    UpdateRepositoryFields,

    /// Backfill GitHub/GitLab stats for crates.
    BackfillRepositoryStats,

    /// Updates info for a crate from the registry's API
    UpdateCrateRegistryFields {
        #[arg(name = "CRATE")]
        name: KrateName,
    },

    /// Remove documentation from the database
    Delete {
        #[command(subcommand)]
        command: DeleteSubcommand,
    },

    /// Compares the database with the index and resolves inconsistencies
    Synchronize {
        /// Don't actually resolve the inconsistencies, just log them
        #[arg(long)]
        dry_run: bool,
    },
}

impl DatabaseSubcommand {
    async fn handle_args(self, config: Arc<Config>, ctx: Context) -> Result<()> {
        match self {
            Self::UpdateRepositoryFields => {
                ctx.repository_stats()?.update_all_crates().await?;
            }

            Self::BackfillRepositoryStats => {
                ctx.repository_stats()?.backfill_repositories().await?;
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

            Self::Delete { command } => command.handle_args(ctx).await?,

            Self::Synchronize { dry_run } => {
                docs_rs_watcher::consistency::run_check(&config, &ctx, dry_run).await?;
            }
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
        name: KrateName,
    },
    /// Delete a single version of a crate (which may include multiple builds)
    Version {
        /// Name of the crate to delete
        #[arg(name = "CRATE_NAME")]
        name: KrateName,

        /// The version of the crate to delete
        #[arg(name = "VERSION")]
        version: Version,
    },
}

impl DeleteSubcommand {
    async fn handle_args(self, ctx: Context) -> Result<()> {
        let mut conn = ctx.pool()?.get_async().await?;
        let storage = ctx.storage()?;

        match self {
            Self::Version { name, version } => {
                docs_rs_watcher::delete_version(&mut conn, storage, &name, &version).await?;
            }

            Self::Crate { name } => {
                docs_rs_watcher::delete_crate(&mut conn, storage, &name).await?;
            }
        }

        Ok(())
    }
}
