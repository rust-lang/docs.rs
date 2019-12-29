

extern crate cratesfyi;
extern crate clap;
extern crate log;
extern crate env_logger;
extern crate time;
extern crate rustwide;


use std::env;
use std::path::{Path, PathBuf};

use clap::{Arg, App, AppSettings, SubCommand};
use cratesfyi::{DocBuilder, RustwideBuilder, DocBuilderOptions, db};
use cratesfyi::utils::add_crate_to_queue;
use cratesfyi::start_web_server;
use cratesfyi::db::{add_path_into_database, connect_db};


pub fn main() {
    logger_init();

    let matches = App::new("cratesfyi")
        .version(cratesfyi::BUILD_VERSION)
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .setting(AppSettings::ArgRequiredElseHelp)
        .subcommand(SubCommand::with_name("build")
            .about("Builds documentation in a chroot environment")
            .setting(AppSettings::ArgRequiredElseHelp)
            .arg(Arg::with_name("PREFIX")
                .short("P")
                .long("prefix")
                .takes_value(true))
            .arg(Arg::with_name("CRATES_IO_INDEX_PATH")
                .long("crates-io-index-path")
                .help("Sets crates.io-index path")
                .takes_value(true))
            .arg(Arg::with_name("SKIP_IF_EXISTS")
                .short("s")
                .long("skip")
                .help("Skips building documentation if documentation exists"))
            .arg(Arg::with_name("SKIP_IF_LOG_EXISTS")
                .long("skip-if-log-exists")
                .help("Skips building documentation if build log exists"))
            .arg(Arg::with_name("KEEP_BUILD_DIRECTORY")
                .short("-k")
                .long("keep-build-directory")
                .help("Keeps build directory after build."))
            .subcommand(SubCommand::with_name("world")
                .about("Builds documentation of every crate"))
            .subcommand(SubCommand::with_name("crate")
                .about("Builds documentation for a crate")
                .arg(Arg::with_name("CRATE_NAME")
                    .index(1)
                    .required_unless("LOCAL")
                    .requires("CRATE_VERSION")
                    .help("Crate name"))
                .arg(Arg::with_name("CRATE_VERSION")
                    .index(2)
                    .help("Version of crate"))
                .arg(Arg::with_name("LOCAL")
                    .short("-l")
                    .long("--local")
                    .takes_value(true)
                    .conflicts_with_all(&["CRATE_NAME", "CRATE_VERSION"])
                    .help("Build a crate at a specific path")))
            .subcommand(SubCommand::with_name("update-toolchain")
                .about("update the curretntly installed rustup toolchain")
                .arg(Arg::with_name("ONLY_FIRST_TIME")
                     .long("only-first-time")
                     .help("update toolchain only if no toolchain is currently installed")))
            .subcommand(SubCommand::with_name("add-essential-files")
                .about("Adds essential files for the installed version of rustc"))
            .subcommand(SubCommand::with_name("lock")
                .about("Locks cratesfyi daemon to stop building new crates"))
            .subcommand(SubCommand::with_name("unlock")
                .about("Unlocks cratesfyi daemon to continue building new crates"))
            .subcommand(SubCommand::with_name("print-options")))
        .subcommand(SubCommand::with_name("start-web-server")
            .about("Starts web server")
            .arg(Arg::with_name("SOCKET_ADDR")
                .index(1)
                .required(false)
                .help("Socket address to listen to")))
        .subcommand(SubCommand::with_name("daemon")
            .about("Starts cratesfyi daemon")
            .arg(Arg::with_name("FOREGROUND")
                .short("-f")
                .long("foreground")
                .help("run the server in the foreground instead of detaching a child")))
        .subcommand(SubCommand::with_name("database")
            .about("Database operations")
            .setting(AppSettings::ArgRequiredElseHelp)
            .subcommand(SubCommand::with_name("move-to-s3"))
            .subcommand(SubCommand::with_name("migrate")
                .about("Run database migrations")
                .arg(Arg::with_name("VERSION")))
            .subcommand(SubCommand::with_name("update-github-fields")
                .about("Updates github stats for crates."))
            .subcommand(SubCommand::with_name("add-directory")
                .about("Adds a directory into database")
                .arg(Arg::with_name("DIRECTORY")
                    .index(1)
                    .required(true)
                    .help("Path of file or directory"))
                .arg(Arg::with_name("PREFIX")
                    .index(2)
                    .help("Prefix of files in database")))
            .subcommand(SubCommand::with_name("update-release-activity")
                .about("Updates monthly release activity chart"))
            .subcommand(SubCommand::with_name("update-search-index")
                .about("Updates search index"))
            .subcommand(SubCommand::with_name("delete-crate")
                .about("Removes a whole crate from the database")
                .arg(Arg::with_name("CRATE_NAME")
                    .takes_value(true)
                    .required(true)
                    .help("Name of the crate to delete")))
            .subcommand(SubCommand::with_name("blacklist")
                .about("Blacklist operations")
                .setting(AppSettings::ArgRequiredElseHelp)
                .subcommand(SubCommand::with_name("list")
                    .about("List all crates on the blacklist"))
                .subcommand(SubCommand::with_name("add")
                    .about("Add a crate to the blacklist")
                    .arg(Arg::with_name("CRATE_NAME")
                        .required(true)
                        .help("Crate name")))
                .subcommand(SubCommand::with_name("remove")
                    .about("Remove a crate from the blacklist")
                    .arg(Arg::with_name("CRATE_NAME")
                        .required(true)
                        .help("Crate name")))))
        .subcommand(SubCommand::with_name("queue")
            .about("Interactions with the build queue")
            .setting(AppSettings::ArgRequiredElseHelp)
            .subcommand(SubCommand::with_name("add")
                .about("Add a crate to the build queue")
                .arg(Arg::with_name("CRATE_NAME")
                    .index(1)
                    .required(true)
                    .help("Name of crate to build"))
                .arg(Arg::with_name("CRATE_VERSION")
                    .index(2)
                    .required(true)
                    .help("Version of crate to build"))
                .arg(Arg::with_name("BUILD_PRIORITY")
                    .short("p")
                    .long("priority")
                    .help("Priority of build (default: 5) (new crate builds get priority 0)")
                    .takes_value(true))))
        .get_matches();



    if let Some(matches) = matches.subcommand_matches("build") {
        let docbuilder_opts = {
            let mut docbuilder_opts = if let Some(prefix) = matches.value_of("PREFIX") {
                DocBuilderOptions::from_prefix(PathBuf::from(prefix))
            } else if let Ok(prefix) = env::var("CRATESFYI_PREFIX") {
                DocBuilderOptions::from_prefix(PathBuf::from(prefix))
            } else {
                DocBuilderOptions::default()
            };

            if let Some(crates_io_index_path) = matches.value_of("CRATES_IO_INDEX_PATH") {
                docbuilder_opts.crates_io_index_path = PathBuf::from(crates_io_index_path);
            }

            docbuilder_opts.skip_if_exists = matches.is_present("SKIP_IF_EXISTS");
            docbuilder_opts.skip_if_log_exists = matches.is_present("SKIP_IF_LOG_EXISTS");
            docbuilder_opts.keep_build_directory = matches.is_present("KEEP_BUILD_DIRECTORY");

            docbuilder_opts.check_paths().unwrap();

            docbuilder_opts
        };

        let mut docbuilder = DocBuilder::new(docbuilder_opts);

        if let Some(_) = matches.subcommand_matches("world") {
            docbuilder.load_cache().expect("Failed to load cache");
            let mut builder = RustwideBuilder::init().unwrap();
            builder.build_world(&mut docbuilder).expect("Failed to build world");
            docbuilder.save_cache().expect("Failed to save cache");
        } else if let Some(matches) = matches.subcommand_matches("crate") {
            docbuilder.load_cache().expect("Failed to load cache");
            let mut builder = RustwideBuilder::init().expect("failed to initialize rustwide");
            match matches.value_of("LOCAL") {
                Some(path) => builder.build_local_package(&mut docbuilder, Path::new(path)),
                None => builder.build_package(&mut docbuilder,
                    matches.value_of("CRATE_NAME").unwrap(), matches.value_of("CRATE_VERSION").unwrap(), None),
            }.expect("Building documentation failed");
            docbuilder.save_cache().expect("Failed to save cache");
        } else if let Some(m) = matches.subcommand_matches("update-toolchain") {
            if m.is_present("ONLY_FIRST_TIME") {
                let conn = db::connect_db().unwrap();
                let res = conn.query("SELECT * FROM config WHERE name = 'rustc_version';", &[]).unwrap();
                if !res.is_empty() {
                    println!("update-toolchain was already called in the past, exiting");
                    return;
                }
            }
            let mut builder = RustwideBuilder::init().unwrap();
            builder.update_toolchain().expect("failed to update toolchain");
        } else if matches.subcommand_matches("add-essential-files").is_some() {
            let mut builder = RustwideBuilder::init().unwrap();
            builder.add_essential_files().expect("failed to add essential files");
        } else if let Some(_) = matches.subcommand_matches("lock") {
            docbuilder.lock().expect("Failed to lock");
        } else if let Some(_) = matches.subcommand_matches("unlock") {
            docbuilder.unlock().expect("Failed to unlock");
        } else if let Some(_) = matches.subcommand_matches("print-options") {
            println!("{:?}", docbuilder.options());
        }

    } else if let Some(matches) = matches.subcommand_matches("database") {
        if let Some(matches) = matches.subcommand_matches("migrate") {
            let version = matches.value_of("VERSION").map(|v| v.parse::<i64>()
                                                          .expect("Version should be an integer"));
            db::migrate(version, &connect_db().expect("failed to connect to the database"))
                .expect("Failed to run database migrations");
        } else if let Some(_) = matches.subcommand_matches("update-github-fields") {
            cratesfyi::utils::github_updater().expect("Failed to update github fields");
        } else if let Some(matches) = matches.subcommand_matches("add-directory") {
            add_path_into_database(&db::connect_db().unwrap(),
                                   matches.value_of("PREFIX").unwrap_or(""),
                                   matches.value_of("DIRECTORY").unwrap())
                .expect("Failed to add directory into database");
        } else if let Some(_) = matches.subcommand_matches("update-release-activity") {
            // FIXME: This is actually util command not database
            cratesfyi::utils::update_release_activity().expect("Failed to update release activity");
        } else if let Some(_) = matches.subcommand_matches("update-search-index") {
            let conn = db::connect_db().unwrap();
            db::update_search_index(&conn).expect("Failed to update search index");
        } else if let Some(_) = matches.subcommand_matches("move-to-s3") {
            let conn = db::connect_db().unwrap();
            let mut count = 1;
            let mut total = 0;
            while count != 0 {
                count = db::file::move_to_s3(&conn, 5_000).expect("Failed to upload batch to S3");
                total += count;
                eprintln!(
                    "moved {} rows to s3 in this batch, total moved so far: {}",
                    count, total
                );
            }
        } else if let Some(matches) = matches.subcommand_matches("delete-crate") {
            let name = matches.value_of("CRATE_NAME").expect("missing crate name");
            let conn = db::connect_db().expect("failed to connect to the database");
            db::delete_crate(&conn, &name).expect("failed to delete the crate");
        } else if let Some(matches) = matches.subcommand_matches("blacklist") {
            let conn = db::connect_db().expect("failed to connect to the database");

            if let Some(_) = matches.subcommand_matches("list") {
                let crates = db::blacklist::list_crates(&conn).expect("failed to list crates on blacklist");
                println!("{}", crates.join("\n"));

            } else if let Some(matches) = matches.subcommand_matches("add") {
                let crate_name = matches.value_of("CRATE_NAME").expect("verified by clap");
                db::blacklist::add_crate(&conn, crate_name).expect("failed to add crate to blacklist");

            } else if let Some(matches) = matches.subcommand_matches("remove") {
                let crate_name = matches.value_of("CRATE_NAME").expect("verified by clap");
                db::blacklist::remove_crate(&conn, crate_name).expect("failed to remove crate from blacklist");
            }
        }

    } else if let Some(matches) = matches.subcommand_matches("start-web-server") {
        start_web_server(Some(matches.value_of("SOCKET_ADDR").unwrap_or("0.0.0.0:3000")));

    } else if let Some(_) = matches.subcommand_matches("daemon") {
        let foreground = matches.subcommand_matches("daemon")
            .map_or(false, |opts| opts.is_present("FOREGROUND"));
        cratesfyi::utils::start_daemon(!foreground);

    } else if let Some(matches) = matches.subcommand_matches("queue") {
        if let Some(matches) = matches.subcommand_matches("add") {
            let priority = matches.value_of("BUILD_PRIORITY").unwrap_or("5");
            let priority: i32 = priority.parse().expect("--priority was not a number");
            let conn = connect_db().expect("Could not connect to database");

            add_crate_to_queue(&conn,
                               matches.value_of("CRATE_NAME").unwrap(),
                               matches.value_of("CRATE_VERSION").unwrap(),
                               priority).expect("Could not add crate to queue");
        }
    }
}



fn logger_init() {
    use std::io::Write;

    let mut builder = env_logger::Builder::new();
    builder.format(|buf, record| {
        writeln!(buf, "{} [{}] {}: {}",
                time::now().strftime("%Y/%m/%d %H:%M:%S").unwrap(),
                record.level(),
                record.target(),
                record.args())
    });
    builder.parse(&env::var("RUST_LOG").unwrap_or("cratesfyi=info".to_owned()));

    rustwide::logging::init_with(builder.build());
}
