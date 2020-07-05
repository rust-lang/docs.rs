use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use cratesfyi::db::{self, add_path_into_database, Pool};
use cratesfyi::utils::{remove_crate_priority, set_crate_priority};
use cratesfyi::{BuildQueue, Config, DocBuilder, DocBuilderOptions, RustwideBuilder, Server};
use failure::Error;
use once_cell::sync::OnceCell;
use structopt::StructOpt;
use strum::VariantNames;

pub fn main() -> Result<(), Error> {
    let _ = dotenv::dotenv();
    logger_init();

    CommandLine::from_args().handle_args()
}

fn logger_init() {
    use std::io::Write;

    let mut builder = env_logger::Builder::new();
    builder.format(|buf, record| {
        writeln!(
            buf,
            "{} [{}] {}: {}",
            time::now().strftime("%Y/%m/%d %H:%M:%S").unwrap(),
            record.level(),
            record.target(),
            record.args()
        )
    });
    builder.parse_filters(
        env::var("RUST_LOG")
            .ok()
            .as_deref()
            .unwrap_or("cratesfyi=info"),
    );

    rustwide::logging::init_with(builder.build());
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
        let ctx = Context::new();

        match self {
            Self::Build(build) => build.handle_args(ctx)?,
            Self::StartWebServer {
                socket_addr,
                reload_templates,
            } => {
                Server::start(
                    Some(&socket_addr),
                    reload_templates,
                    ctx.pool()?,
                    ctx.config()?,
                    ctx.build_queue()?,
                )?;
            }
            Self::Daemon {
                foreground,
                registry_watcher,
            } => {
                if foreground {
                    log::warn!("--foreground was passed, but there is no need for it anymore");
                }

                cratesfyi::utils::start_daemon(
                    ctx.config()?,
                    ctx.pool()?,
                    ctx.build_queue()?,
                    registry_watcher == Toggle::Enabled,
                )?;
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
    pub fn handle_args(self, ctx: Context) -> Result<(), Error> {
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
    pub fn handle_args(self, ctx: Context) -> Result<(), Error> {
        match self {
            Self::Set { pattern, priority } => {
                set_crate_priority(&*ctx.conn()?, &pattern, priority)
                    .expect("Could not set pattern's priority");
            }

            Self::Remove { pattern } => {
                if let Some(priority) = remove_crate_priority(&*ctx.conn()?, &pattern)
                    .expect("Could not remove pattern's priority")
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
    #[structopt(
        name = "PREFIX",
        short = "P",
        long = "prefix",
        env = "CRATESFYI_PREFIX"
    )]
    prefix: PathBuf,

    /// Sets the registry index path, where on disk the registry index will be cloned to
    #[structopt(
        name = "REGISTRY_INDEX_PATH",
        long = "registry-index-path",
        alias = "crates-io-index-path"
    )]
    registry_index_path: Option<PathBuf>,

    /// Skips building documentation if documentation exists
    #[structopt(name = "SKIP_IF_EXISTS", short = "s", long = "skip")]
    skip_if_exists: bool,

    /// Skips building documentation if build log exists
    #[structopt(name = "SKIP_IF_LOG_EXISTS", long = "skip-if-log-exists")]
    skip_if_log_exists: bool,

    /// Keeps build directory after build.
    #[structopt(
        name = "KEEP_BUILD_DIRECTORY",
        short = "k",
        long = "keep-build-directory"
    )]
    keep_build_directory: bool,

    #[structopt(subcommand)]
    subcommand: BuildSubcommand,
}

impl Build {
    pub fn handle_args(self, ctx: Context) -> Result<(), Error> {
        let docbuilder = {
            let mut doc_options = DocBuilderOptions::from_prefix(self.prefix);

            if let Some(registry_index_path) = self.registry_index_path {
                doc_options.registry_index_path = registry_index_path;
            }

            doc_options.skip_if_exists = self.skip_if_exists;
            doc_options.skip_if_log_exists = self.skip_if_log_exists;
            doc_options.keep_build_directory = self.keep_build_directory;

            doc_options
                .check_paths()
                .expect("The given paths were invalid");

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
    pub fn handle_args(self, ctx: Context, mut docbuilder: DocBuilder) -> Result<(), Error> {
        match self {
            Self::World => {
                docbuilder.load_cache().expect("Failed to load cache");

                let mut builder = RustwideBuilder::init(ctx.pool()?).unwrap();
                builder
                    .build_world(&mut docbuilder)
                    .expect("Failed to build world");

                docbuilder.save_cache().expect("Failed to save cache");
            }

            Self::Crate {
                crate_name,
                crate_version,
                local,
            } => {
                docbuilder.load_cache().expect("Failed to load cache");
                let mut builder =
                    RustwideBuilder::init(ctx.pool()?).expect("failed to initialize rustwide");

                if let Some(path) = local {
                    builder
                        .build_local_package(&mut docbuilder, &path)
                        .expect("Building documentation failed");
                } else {
                    builder
                        .build_package(
                            &mut docbuilder,
                            &crate_name.unwrap(),
                            &crate_version.unwrap(),
                            None,
                        )
                        .expect("Building documentation failed");
                }

                docbuilder.save_cache().expect("Failed to save cache");
            }

            Self::UpdateToolchain { only_first_time } => {
                if only_first_time {
                    let conn = ctx
                        .pool()?
                        .get()
                        .expect("failed to get a database connection");
                    let res = conn
                        .query("SELECT * FROM config WHERE name = 'rustc_version';", &[])
                        .unwrap();

                    if !res.is_empty() {
                        println!("update-toolchain was already called in the past, exiting");
                        return Ok(());
                    }
                }

                let mut builder = RustwideBuilder::init(ctx.pool()?).unwrap();
                builder
                    .update_toolchain()
                    .expect("failed to update toolchain");
            }

            Self::AddEssentialFiles => {
                let mut builder = RustwideBuilder::init(ctx.pool()?).unwrap();
                builder
                    .add_essential_files()
                    .expect("failed to add essential files");
            }

            Self::Lock => docbuilder.lock().expect("Failed to lock"),
            Self::Unlock => docbuilder.unlock().expect("Failed to unlock"),
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

    /// Removes a whole crate from the database
    DeleteCrate {
        /// Name of the crate to delete
        #[structopt(name = "CRATE_NAME")]
        crate_name: String,
    },

    /// Blacklist operations
    Blacklist {
        #[structopt(subcommand)]
        command: BlacklistSubcommand,
    },
}

impl DatabaseSubcommand {
    pub fn handle_args(self, ctx: Context) -> Result<(), Error> {
        match self {
            Self::Migrate { version } => {
                db::migrate(version, &*ctx.conn()?).expect("Failed to run database migrations");
            }

            Self::UpdateGithubFields => {
                cratesfyi::utils::GithubUpdater::new(&*ctx.config()?, ctx.pool()?)?
                    .update_all_crates()?;
            }

            Self::AddDirectory { directory, prefix } => {
                add_path_into_database(&*ctx.conn()?, &prefix, directory)
                    .expect("Failed to add directory into database");
            }

            // FIXME: This is actually util command not database
            Self::UpdateReleaseActivity => cratesfyi::utils::update_release_activity(&*ctx.conn()?)
                .expect("Failed to update release activity"),

            Self::DeleteCrate { crate_name } => {
                db::delete_crate(&*ctx.conn()?, &crate_name).expect("failed to delete the crate");
            }

            Self::Blacklist { command } => command.handle_args(ctx)?,
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
    fn handle_args(self, ctx: Context) -> Result<(), Error> {
        let conn = &*ctx.conn()?;
        match self {
            Self::List => {
                let crates =
                    db::blacklist::list_crates(&conn).expect("failed to list crates on blacklist");

                println!("{}", crates.join("\n"));
            }

            Self::Add { crate_name } => db::blacklist::add_crate(&conn, &crate_name)
                .expect("failed to add crate to blacklist"),

            Self::Remove { crate_name } => db::blacklist::remove_crate(&conn, &crate_name)
                .expect("failed to remove crate from blacklist"),
        }
        Ok(())
    }
}

struct Context {
    build_queue: OnceCell<Arc<BuildQueue>>,
    config: OnceCell<Arc<Config>>,
    pool: OnceCell<Pool>,
}

impl Context {
    fn new() -> Self {
        Self {
            build_queue: OnceCell::new(),
            config: OnceCell::new(),
            pool: OnceCell::new(),
        }
    }

    fn build_queue(&self) -> Result<Arc<BuildQueue>, Error> {
        Ok(self
            .build_queue
            .get_or_try_init::<_, Error>(|| {
                Ok(Arc::new(BuildQueue::new(self.pool()?, &*self.config()?)))
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
            .get_or_try_init::<_, Error>(|| Ok(Pool::new(&*self.config()?)?))?
            .clone())
    }

    fn conn(
        &self,
    ) -> Result<r2d2::PooledConnection<r2d2_postgres::PostgresConnectionManager>, Error> {
        Ok(self.pool()?.get()?)
    }
}
