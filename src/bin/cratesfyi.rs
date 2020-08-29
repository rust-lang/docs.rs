use std::env;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::Arc;

use cratesfyi::db::{self, add_path_into_database, Pool, PoolClient};
use cratesfyi::index::Index;
use cratesfyi::utils::{remove_crate_priority, set_crate_priority};
use cratesfyi::{
    BuildQueue, Config, Context, DocBuilder, DocBuilderOptions, Metrics, RustwideBuilder, Server,
    Storage,
};
use failure::{err_msg, Error, ResultExt};
use once_cell::sync::OnceCell;
use structopt::StructOpt;
use strum::VariantNames;

pub fn main() {
    let _ = dotenv::dotenv();
    logger_init();

    if let Err(err) = CommandLine::from_args().handle_args() {
        let mut msg = format!("Error: {}", err);
        for cause in err.iter_causes() {
            write!(msg, "\n\nCaused by:\n    {}", cause).unwrap();
        }
        eprintln!("{}", msg);
        if !err.backtrace().is_empty() {
            eprintln!("\nStack backtrace:\n{}", err.backtrace());
        }
        std::process::exit(1);
    }
}

fn logger_init() {
    use std::io::Write;

    let env = env_logger::Env::default().filter_or("DOCSRS_LOG", "cratesfyi=info");
    let logger = env_logger::from_env(env)
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
        .build();

    rustwide::logging::init_with(logger);
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
    version = cratesfyi::BUILD_VERSION,
    rename_all = "kebab-case",
)]
enum CommandLine {
    Build(Build),

    /// Starts web server
    StartWebServer {
        #[structopt(name = "SOCKET_ADDR", default_value = "0.0.0.0:3000")]
        socket_addr: String,

        /// Reload templates when they're changed
        #[structopt(long = "reload-templates")]
        reload_templates: bool,
    },

    /// Starts cratesfyi daemon
    Daemon {
        /// Deprecated. Run the server in the foreground instead of detaching a child
        #[structopt(name = "FOREGROUND", short = "f", long = "foreground")]
        foreground: bool,

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
    pub fn handle_args(self) -> Result<(), Error> {
        let ctx = BinContext::new();

        match self {
            Self::Build(build) => build.handle_args(ctx)?,
            Self::StartWebServer {
                socket_addr,
                reload_templates,
            } => {
                Server::start(Some(&socket_addr), reload_templates, &ctx)?;
            }
            Self::Daemon {
                foreground,
                registry_watcher,
            } => {
                if foreground {
                    log::warn!("--foreground was passed, but there is no need for it anymore");
                }

                cratesfyi::utils::start_daemon(&ctx, registry_watcher == Toggle::Enabled)?;
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
    pub fn handle_args(self, ctx: BinContext) -> Result<(), Error> {
        match self {
            Self::Add {
                crate_name,
                crate_version,
                build_priority,
            } => ctx
                .build_queue()?
                .add_crate(&crate_name, &crate_version, build_priority)?,

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
    pub fn handle_args(self, ctx: BinContext) -> Result<(), Error> {
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

    /// Skips building documentation if build log exists
    #[structopt(name = "SKIP_IF_LOG_EXISTS", long = "skip-if-log-exists")]
    skip_if_log_exists: bool,

    #[structopt(subcommand)]
    subcommand: BuildSubcommand,
}

impl Build {
    pub fn handle_args(self, ctx: BinContext) -> Result<(), Error> {
        let docbuilder = {
            let config = ctx.config()?;
            let mut doc_options =
                DocBuilderOptions::new(config.prefix.clone(), config.registry_index_path.clone());

            doc_options.skip_if_exists = self.skip_if_exists;

            doc_options
                .check_paths()
                .context("The given paths were invalid")?;

            DocBuilder::new(doc_options, ctx.pool()?, ctx.build_queue()?)
        };

        self.subcommand.handle_args(ctx, docbuilder)
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

    /// Locks cratesfyi daemon to stop building new crates
    Lock,

    /// Unlocks cratesfyi daemon to continue building new crates
    Unlock,

    PrintOptions,
}

impl BuildSubcommand {
    pub fn handle_args(self, ctx: BinContext, mut docbuilder: DocBuilder) -> Result<(), Error> {
        let rustwide_builder = || -> Result<RustwideBuilder, Error> {
            Ok(RustwideBuilder::init(
                ctx.config()?,
                ctx.pool()?,
                ctx.metrics()?,
                ctx.storage()?,
            )?)
        };

        match self {
            Self::World => {
                rustwide_builder()?
                    .build_world(&mut docbuilder)
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
                        .build_local_package(&mut docbuilder, &path)
                        .context("Building documentation failed")?;
                } else {
                    builder
                        .build_package(
                            &mut docbuilder,
                            &crate_name.ok_or_else(|| err_msg("must specify name if not local"))?,
                            &crate_version
                                .ok_or_else(|| err_msg("must specify version if not local"))?,
                            None,
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
                    let res =
                        conn.query("SELECT * FROM config WHERE name = 'rustc_version';", &[])?;

                    if !res.is_empty() {
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

            Self::Lock => docbuilder.lock().context("Failed to lock")?,
            Self::Unlock => docbuilder.unlock().context("Failed to unlock")?,
            Self::PrintOptions => println!("{:?}", docbuilder.options()),
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

    /// Updates github stats for crates.
    UpdateGithubFields,

    /// Updates info for a crate from the registry's API
    UpdateCrateRegistryFields {
        #[structopt(name = "CRATE")]
        name: String,
    },

    AddDirectory {
        /// Path of file or directory
        #[structopt(name = "DIRECTORY")]
        directory: PathBuf,
        /// Prefix of files in database
        #[structopt(name = "PREFIX", env = "CRATESFYI_PREFIX")]
        prefix: String,
    },

    /// Updates monthly release activity chart
    UpdateReleaseActivity,

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
    Synchronize {
        /// Don't actually resolve the inconsistencies, just log them
        #[structopt(long)]
        dry_run: bool,
    },
}

impl DatabaseSubcommand {
    pub fn handle_args(self, ctx: BinContext) -> Result<(), Error> {
        match self {
            Self::Migrate { version } => {
                db::migrate(version, &mut *ctx.conn()?)
                    .context("Failed to run database migrations")?;
            }

            Self::UpdateGithubFields => {
                cratesfyi::utils::GithubUpdater::new(&*ctx.config()?, ctx.pool()?)?
                    .update_all_crates()?;
            }

            Self::UpdateCrateRegistryFields { name } => {
                let index = Index::new(&ctx.config()?.registry_index_path)?;

                db::update_crate_data_in_database(
                    &mut *ctx.conn()?,
                    &name,
                    &index.api().get_crate_data(&name)?,
                )?;
            }

            Self::AddDirectory { directory, prefix } => {
                add_path_into_database(&*ctx.storage()?, &prefix, directory)
                    .context("Failed to add directory into database")?;
            }

            // FIXME: This is actually util command not database
            Self::UpdateReleaseActivity => {
                cratesfyi::utils::update_release_activity(&mut *ctx.conn()?)
                    .context("Failed to update release activity")?
            }

            Self::Delete {
                command: DeleteSubcommand::Version { name, version },
            } => db::delete_version(&mut *ctx.conn()?, &*ctx.storage()?, &name, &version)
                .context("failed to delete the crate")?,
            Self::Delete {
                command: DeleteSubcommand::Crate { name },
            } => db::delete_crate(&mut *ctx.conn()?, &*ctx.storage()?, &name)
                .context("failed to delete the crate")?,
            Self::Blacklist { command } => command.handle_args(ctx)?,

            Self::Synchronize { dry_run } => {
                cratesfyi::utils::consistency::run_check(
                    &*ctx.config()?,
                    &mut *ctx.conn()?,
                    dry_run,
                )?;
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
    fn handle_args(self, ctx: BinContext) -> Result<(), Error> {
        let mut conn = &mut *ctx.conn()?;
        match self {
            Self::List => {
                let crates = db::blacklist::list_crates(&mut conn)
                    .context("failed to list crates on blacklist")?;

                println!("{}", crates.join("\n"));
            }

            Self::Add { crate_name } => db::blacklist::add_crate(&mut conn, &crate_name)
                .context("failed to add crate to blacklist")?,

            Self::Remove { crate_name } => db::blacklist::remove_crate(&mut conn, &crate_name)
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
}

impl BinContext {
    fn new() -> Self {
        Self {
            build_queue: OnceCell::new(),
            storage: OnceCell::new(),
            config: OnceCell::new(),
            pool: OnceCell::new(),
            metrics: OnceCell::new(),
        }
    }

    fn conn(&self) -> Result<PoolClient, Error> {
        Ok(self.pool()?.get()?)
    }
}

impl Context for BinContext {
    fn build_queue(&self) -> Result<Arc<BuildQueue>, Error> {
        Ok(self
            .build_queue
            .get_or_try_init::<_, Error>(|| {
                Ok(Arc::new(BuildQueue::new(
                    self.pool()?,
                    self.metrics()?,
                    &*self.config()?,
                )))
            })?
            .clone())
    }

    fn storage(&self) -> Result<Arc<Storage>, Error> {
        Ok(self
            .storage
            .get_or_try_init::<_, Error>(|| {
                Ok(Arc::new(Storage::new(
                    self.pool()?,
                    self.metrics()?,
                    &*self.config()?,
                )?))
            })?
            .clone())
    }

    fn config(&self) -> Result<Arc<Config>, Error> {
        Ok(self
            .config
            .get_or_try_init::<_, Error>(|| Ok(Arc::new(Config::from_env()?)))?
            .clone())
    }

    fn pool(&self) -> Result<Pool, Error> {
        Ok(self
            .pool
            .get_or_try_init::<_, Error>(|| Ok(Pool::new(&*self.config()?, self.metrics()?)?))?
            .clone())
    }

    fn metrics(&self) -> Result<Arc<Metrics>, Error> {
        Ok(self
            .metrics
            .get_or_try_init::<_, Error>(|| Ok(Arc::new(Metrics::new()?)))?
            .clone())
    }
}
