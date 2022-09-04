use std::env;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Error, Result};
use docs_rs::db::{self, add_path_into_database, Pool, PoolClient};
use docs_rs::repositories::RepositoryStatsUpdater;
use docs_rs::utils::{
    get_config, queue_builder, remove_crate_priority, set_crate_priority, ConfigName,
};
use docs_rs::{
    BuildQueue, Config, Context, Index, Metrics, PackageKind, RustwideBuilder, Server, Storage,
};
use once_cell::sync::OnceCell;
use sentry_log::SentryLogger;
use structopt::StructOpt;
use strum::VariantNames;

fn main() {
    let _sentry_guard = if let Ok(sentry_dsn) = env::var("SENTRY_DSN") {
        rustwide::logging::init_with(SentryLogger::with_dest(logger_init()));
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
        rustwide::logging::init_with(logger_init());
        None
    };

    if let Err(err) = CommandLine::from_args().handle_args() {
        let mut msg = format!("Error: {}", err);
        for cause in err.chain() {
            write!(msg, "\n\nCaused by:\n    {}", cause).unwrap();
        }
        eprintln!("{}", msg);

        let backtrace = err.backtrace().to_string();
        if !backtrace.is_empty() {
            eprintln!("\nStack backtrace:\n{}", backtrace);
        }

        // we need to drop the sentry guard here so all unsent
        // errors are sent to sentry before
        // process::exit kills everything.
        drop(_sentry_guard);
        std::process::exit(1);
    }
}

fn logger_init() -> env_logger::Logger {
    use std::io::Write;

    let env = env_logger::Env::default().filter_or("DOCSRS_LOG", "docs_rs=info");
    env_logger::Builder::from_env(env)
        .format(|buf, record| {
            writeln!(
                buf,
                "{} [{}] {}: {}",
                time::now().strftime("%Y/%m/%d %H:%M:%S").unwrap(),
                record.level(),
                record.target(),
                record.args()
            )
        })
        .build()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString, strum::EnumVariantNames)]
#[strum(serialize_all = "snake_case")]
enum Toggle {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, StructOpt)]
#[structopt(
    name = "cratesfyi",
    about = env!("CARGO_PKG_DESCRIPTION"),
    version = docs_rs::BUILD_VERSION,
    rename_all = "kebab-case",
)]
enum CommandLine {
    Build(Build),

    /// Starts web server
    StartWebServer {
        #[structopt(name = "SOCKET_ADDR", default_value = "0.0.0.0:3000")]
        socket_addr: String,
    },

    StartRegistryWatcher {
        /// Enable or disable the repository stats updater
        #[structopt(
            long = "repository-stats-updater",
            default_value = "disabled",
            possible_values(Toggle::VARIANTS)
        )]
        repository_stats_updater: Toggle,
    },

    StartBuildServer,

    /// Starts the daemon
    Daemon {
        /// Enable or disable the registry watcher to automatically enqueue newly published crates
        #[structopt(
            long = "registry-watcher",
            default_value = "enabled",
            possible_values(Toggle::VARIANTS)
        )]
        registry_watcher: Toggle,
    },

    /// Database operations
    Database {
        #[structopt(subcommand)]
        subcommand: DatabaseSubcommand,
    },

    /// Interactions with the build queue
    Queue {
        #[structopt(subcommand)]
        subcommand: QueueSubcommand,
    },
}

impl CommandLine {
    fn handle_args(self) -> Result<()> {
        let ctx = BinContext::new();

        match self {
            Self::Build(build) => build.handle_args(ctx)?,
            Self::StartRegistryWatcher {
                repository_stats_updater,
            } => {
                if repository_stats_updater == Toggle::Enabled {
                    docs_rs::utils::daemon::start_background_repository_stats_updater(&ctx)?;
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
                let _ = Server::start(Some(&socket_addr), &ctx)?;
            }
            Self::Daemon { registry_watcher } => {
                docs_rs::utils::start_daemon(&ctx, registry_watcher == Toggle::Enabled)?;
            }
            Self::Database { subcommand } => subcommand.handle_args(ctx)?,
            Self::Queue { subcommand } => subcommand.handle_args(ctx)?,
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StructOpt)]
enum QueueSubcommand {
    /// Add a crate to the build queue
    Add {
        /// Name of crate to build
        #[structopt(name = "CRATE_NAME")]
        crate_name: String,
        /// Version of crate to build
        #[structopt(name = "CRATE_VERSION")]
        crate_version: String,
        /// Priority of build (new crate builds get priority 0)
        #[structopt(
            name = "BUILD_PRIORITY",
            short = "p",
            long = "priority",
            default_value = "5"
        )]
        build_priority: i32,
    },

    /// Interactions with build queue priorities
    DefaultPriority {
        #[structopt(subcommand)]
        subcommand: PrioritySubcommand,
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

            Self::DefaultPriority { subcommand } => subcommand.handle_args(ctx)?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StructOpt)]
enum PrioritySubcommand {
    /// Set all crates matching a pattern to a priority level
    Set {
        /// See https://www.postgresql.org/docs/current/functions-matching.html for pattern syntax
        #[structopt(name = "PATTERN")]
        pattern: String,
        /// The priority to give crates matching the given `PATTERN`
        priority: i32,
    },

    /// Remove the prioritization of crates for a pattern
    Remove {
        /// See https://www.postgresql.org/docs/current/functions-matching.html for pattern syntax
        #[structopt(name = "PATTERN")]
        pattern: String,
    },
}

impl PrioritySubcommand {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        match self {
            Self::Set { pattern, priority } => {
                set_crate_priority(&mut *ctx.conn()?, &pattern, priority)
                    .context("Could not set pattern's priority")?;
            }

            Self::Remove { pattern } => {
                if let Some(priority) = remove_crate_priority(&mut *ctx.conn()?, &pattern)
                    .context("Could not remove pattern's priority")?
                {
                    println!("Removed pattern with priority {}", priority);
                } else {
                    println!("Pattern did not exist and so was not removed");
                }
            }
        }
        Ok(())
    }
}

/// Builds documentation in a chroot environment
#[derive(Debug, Clone, PartialEq, Eq, StructOpt)]
#[structopt(rename_all = "kebab-case")]
struct Build {
    /// Skips building documentation if documentation exists
    #[structopt(name = "SKIP_IF_EXISTS", short = "s", long = "skip")]
    skip_if_exists: bool,

    #[structopt(subcommand)]
    subcommand: BuildSubcommand,
}

impl Build {
    fn handle_args(self, ctx: BinContext) -> Result<()> {
        self.subcommand.handle_args(ctx, self.skip_if_exists)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StructOpt)]
enum BuildSubcommand {
    /// Builds documentation of every crate
    World,

    /// Builds documentation for a crate
    Crate {
        /// Crate name
        #[structopt(
            name = "CRATE_NAME",
            required_unless("local"),
            requires("CRATE_VERSION")
        )]
        crate_name: Option<String>,

        /// Version of crate
        #[structopt(name = "CRATE_VERSION")]
        crate_version: Option<String>,

        /// Build a crate at a specific path
        #[structopt(short = "l", long = "local", conflicts_with_all(&["CRATE_NAME", "CRATE_VERSION"]))]
        local: Option<PathBuf>,
    },

    /// update the currently installed rustup toolchain
    UpdateToolchain {
        /// Update the toolchain only if no toolchain is currently installed
        #[structopt(name = "ONLY_FIRST_TIME", long = "only-first-time")]
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

                    if get_config(&mut conn, ConfigName::RustcVersion)?.is_string() {
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

#[derive(Debug, Clone, PartialEq, Eq, StructOpt)]
enum DatabaseSubcommand {
    /// Run database migration
    Migrate {
        /// The database version to migrate to
        #[structopt(name = "VERSION")]
        version: Option<i64>,
    },

    /// Updates Github/Gitlab stats for crates.
    UpdateRepositoryFields,

    /// Backfill GitHub/Gitlab stats for crates.
    BackfillRepositoryStats,

    /// Updates info for a crate from the registry's API
    UpdateCrateRegistryFields {
        #[structopt(name = "CRATE")]
        name: String,
    },

    AddDirectory {
        /// Path of file or directory
        #[structopt(name = "DIRECTORY")]
        directory: PathBuf,
    },

    /// Remove documentation from the database
    Delete {
        #[structopt(subcommand)]
        command: DeleteSubcommand,
    },

    /// Blacklist operations
    Blacklist {
        #[structopt(subcommand)]
        command: BlacklistSubcommand,
    },

    /// Compares the database with the index and resolves inconsistencies
    #[cfg(feature = "consistency_check")]
    Synchronize {
        /// Don't actually resolve the inconsistencies, just log them
        #[structopt(long)]
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

            #[cfg(feature = "consistency_check")]
            Self::Synchronize { dry_run } => {
                docs_rs::utils::consistency::run_check(&mut *ctx.conn()?, &*ctx.index()?, dry_run)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StructOpt)]
enum BlacklistSubcommand {
    /// List all crates on the blacklist
    List,

    /// Add a crate to the blacklist
    Add {
        /// Crate name
        #[structopt(name = "CRATE_NAME")]
        crate_name: String,
    },

    /// Remove a crate from the blacklist
    Remove {
        /// Crate name
        #[structopt(name = "CRATE_NAME")]
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

#[derive(Debug, Clone, PartialEq, Eq, StructOpt)]
enum DeleteSubcommand {
    /// Delete a whole crate
    Crate {
        /// Name of the crate to delete
        #[structopt(name = "CRATE_NAME")]
        name: String,
    },
    /// Delete a single version of a crate (which may include multiple builds)
    Version {
        /// Name of the crate to delete
        #[structopt(name = "CRATE_NAME")]
        name: String,

        /// The version of the crate to delete
        #[structopt(name = "VERSION")]
        version: String,
    },
}

struct BinContext {
    build_queue: OnceCell<Arc<BuildQueue>>,
    storage: OnceCell<Arc<Storage>>,
    config: OnceCell<Arc<Config>>,
    pool: OnceCell<Pool>,
    metrics: OnceCell<Arc<Metrics>>,
    index: OnceCell<Arc<Index>>,
    repository_stats_updater: OnceCell<Arc<RepositoryStatsUpdater>>,
}

impl BinContext {
    fn new() -> Self {
        Self {
            build_queue: OnceCell::new(),
            storage: OnceCell::new(),
            config: OnceCell::new(),
            pool: OnceCell::new(),
            metrics: OnceCell::new(),
            index: OnceCell::new(),
            repository_stats_updater: OnceCell::new(),
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
        )?;
        fn config(self) -> Config = Config::from_env()?;
        fn metrics(self) -> Metrics = Metrics::new()?;
        fn index(self) -> Index = {
            let config = self.config()?;
            let path = config.registry_index_path.clone();
            if let Some(registry_url) = config.registry_url.clone() {
                Index::from_url(path, registry_url)
            } else {
                Index::new(path)
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
