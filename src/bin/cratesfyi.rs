

extern crate cratesfyi;
extern crate clap;
#[macro_use]
extern crate log;
extern crate env_logger;
extern crate time;


use std::env;
use std::process;
use std::path::PathBuf;

use clap::{Arg, App, SubCommand};
use cratesfyi::{DocBuilder, DocBuilderOptions, db};
use cratesfyi::utils::build_doc;
use cratesfyi::start_web_server;
use cratesfyi::db::add_path_into_database;


pub fn main() {
    logger_init();

    let matches = App::new("cratesfyi")
                      .version(cratesfyi::BUILD_VERSION)
                      .about(env!("CARGO_PKG_DESCRIPTION"))
                      .subcommand(SubCommand::with_name("doc")
                                      .about("Builds documentation of a crate")
                                      .arg(Arg::with_name("CRATE_NAME")
                                               .index(1)
                                               .required(true)
                                               .help("Crate name"))
                                      .arg(Arg::with_name("CRATE_VERSION")
                                               .index(2)
                                               .required(false)
                                               .help("Crate version"))
                                      .arg(Arg::with_name("TARGET")
                                               .index(3)
                                               .required(false)
                                               .help("The target platform to compile for")))
                      .subcommand(SubCommand::with_name("build")
                                      .about("Builds documentation in a chroot environment")
                                      .arg(Arg::with_name("PREFIX")
                                               .short("P")
                                               .long("prefix")
                                               .takes_value(true))
                                      .arg(Arg::with_name("DESTINATION")
                                               .short("d")
                                               .long("destination")
                                               .help("Sets destination path")
                                               .takes_value(true))
                                      .arg(Arg::with_name("CHROOT_PATH")
                                               .short("c")
                                               .long("chroot")
                                               .help("Sets chroot path")
                                               .takes_value(true))
                                      .arg(Arg::with_name("CHROOT_USER")
                                               .short("u")
                                               .long("chroot-user")
                                               .help("Sets chroot user name")
                                               .takes_value(true))
                                      .arg(Arg::with_name("CRATES_IO_INDEX_PATH")
                                               .long("crates-io-index-path")
                                               .help("Sets crates.io-index path")
                                               .takes_value(true))
                                      .arg(Arg::with_name("SKIP_IF_EXISTS")
                                               .short("s")
                                               .long("skip")
                                               .help("Skips building documentation if \
                                                      documentation exists"))
                                      .arg(Arg::with_name("SKIP_IF_LOG_EXISTS")
                                               .long("skip-if-log-exists")
                                               .help("Skips building documentation if build \
                                                      log exists"))
                                      .arg(Arg::with_name("KEEP_BUILD_DIRECTORY")
                                               .short("-k")
                                               .long("keep-build-directory")
                                               .help("Keeps build directory after build."))
                                      .subcommand(SubCommand::with_name("world")
                                                      .about("Builds documentation of every \
                                                              crate"))
                                      .subcommand(SubCommand::with_name("crate")
                                                      .about("Builds documentation for a crate")
                                                      .arg(Arg::with_name("CRATE_NAME")
                                                               .index(1)
                                                               .required(true)
                                                               .help("Crate name"))
                                                      .arg(Arg::with_name("CRATE_VERSION")
                                                               .index(2)
                                                               .required(true)
                                                               .help("Version of crate")))
                                      .subcommand(SubCommand::with_name("add-essential-files")
                                                      .about("Adds essential files for rustc")))
                      .subcommand(SubCommand::with_name("start-web-server")
                                      .about("Starts web server")
                                      .arg(Arg::with_name("SOCKET_ADDR")
                                               .index(1)
                                               .required(false)
                                               .help("Socket address to listen to")))
                      .subcommand(SubCommand::with_name("daemon")
                                      .about("Starts cratesfyi daemon"))
                      .subcommand(SubCommand::with_name("database")
                                      .about("Database operations")
                                      .subcommand(SubCommand::with_name("init")
                                                      .about("Initialize database. Currently \
                                                              only creates tables in database."))
                                      .subcommand(SubCommand::with_name("update-github-fields")
                                                      .about("Updates github stats for crates."))
                                      .subcommand(SubCommand::with_name("add-directory")
                                                      .about("Adds a directory into database")
                                                      .arg(Arg::with_name("DIRECTORY")
                                                               .index(1)
                                                               .required(true)
                                                               .help("Path of file or \
                                                                      directory"))
                                                      .arg(Arg::with_name("PREFIX")
                                                               .index(2)
                                                               .help("Prefix of files in \
                                                                      database")))
                                      .subcommand(SubCommand::with_name("update-release-activity")))
                      .get_matches();



    // doc subcommand
    if let Some(matches) = matches.subcommand_matches("doc") {
        let name = matches.value_of("CRATE_NAME").unwrap();
        let version = matches.value_of("CRATE_VERSION");
        let target = matches.value_of("TARGET");
        if let Err(e) = build_doc(name, version, target) {
            panic!("{:#?}", e);
        }
    } else if let Some(matches) = matches.subcommand_matches("build") {
        let docbuilder_opts = {
            let mut docbuilder_opts = if let Some(prefix) = matches.value_of("PREFIX") {
                DocBuilderOptions::from_prefix(PathBuf::from(prefix))
            } else if let Ok(prefix) = env::var("CRATESFYI_PREFIX") {
                DocBuilderOptions::from_prefix(PathBuf::from(prefix))
            } else {
                DocBuilderOptions::default()
            };

            // set options
            if let Some(destination) = matches.value_of("DESTINATION") {
                docbuilder_opts.destination = PathBuf::from(destination);
            }

            if let Some(chroot_path) = matches.value_of("CHROOT_PATH") {
                docbuilder_opts.chroot_path = PathBuf::from(chroot_path);
            }

            if let Some(chroot_user) = matches.value_of("CHROOT_USER") {
                docbuilder_opts.chroot_user = chroot_user.to_string();
            }

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

        docbuilder.load_cache().expect("Failed to load cache");

        if let Some(_) = matches.subcommand_matches("world") {
            docbuilder.build_world().expect("Failed to build world");
        } else if let Some(matches) = matches.subcommand_matches("crate") {
            docbuilder.build_package(matches.value_of("CRATE_NAME").unwrap(),
                                     matches.value_of("CRATE_VERSION").unwrap())
                      .expect("Building documentation failed");
        } else if let Some(_) = matches.subcommand_matches("add-essential-files") {
            docbuilder.add_essential_files().expect("Failed to add essential files");
        }

        docbuilder.save_cache().expect("Failed to save cache");
    } else if let Some(matches) = matches.subcommand_matches("database") {
        if let Some(_) = matches.subcommand_matches("init") {
            use std::io::Write;
            use std::io;
            let conn = db::connect_db().unwrap();
            if let Err(err) = db::create_tables(&conn) {
                writeln!(&mut io::stderr(), "Failed to initialize database: {}", err).unwrap();
                process::exit(1);
            }
        } else if let Some(_) = matches.subcommand_matches("update-github-fields") {
            cratesfyi::utils::github_updater().expect("Failed to update github fields");
        } else if let Some(matches) = matches.subcommand_matches("add-directory") {
            add_path_into_database(&db::connect_db().unwrap(),
                                   matches.value_of("PREFIX").unwrap_or(""),
                                   matches.value_of("DIRECTORY").unwrap())
                .unwrap();
        } else if let Some(_) = matches.subcommand_matches("update-release-activity") {
            // FIXME: This is actually util command not database
            cratesfyi::utils::update_release_activity().expect("Failed to update release activity");
        }
    } else if let Some(matches) = matches.subcommand_matches("start-web-server") {
        start_web_server(Some(matches.value_of("SOCKET_ADDR").unwrap_or("0.0.0.0:3000")));
    } else if let Some(_) = matches.subcommand_matches("daemon") {
        cratesfyi::utils::start_daemon();
    } else {
        println!("{}", matches.usage());
    }
}



fn logger_init() {
    let format = |record: &log::LogRecord| {
        format!("{} [{}] {}: {}",
                time::now().strftime("%Y/%m/%d %H:%M:%S").unwrap(),
                record.level(), record.target(), record.args())
    };

    let mut builder = env_logger::LogBuilder::new();
    builder.format(format);
    builder.parse(&env::var("RUST_LOG").unwrap_or("cratesfyi=info".to_owned()));
    builder.init().unwrap();
}
