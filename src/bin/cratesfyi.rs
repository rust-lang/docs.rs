

extern crate cratesfyi;
extern crate clap;
#[macro_use]
extern crate log;
extern crate env_logger;


use std::env;
use std::process;
use std::path::PathBuf;

use clap::{Arg, App, SubCommand};
use cratesfyi::{DocBuilder, DocBuilderOptions, db};
use cratesfyi::utils::build_doc;


pub fn main() {
    let _ = env_logger::init();

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
                                               .help("Crate version")))
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
                                                               .help("Version of crate"))))
                      .subcommand(SubCommand::with_name("database")
                                      .about("Database operations")
                                      .subcommand(SubCommand::with_name("init")
                                                      .about("Initialize database. Currently \
                                                             only creates tables in database.")))
                      .get_matches();



    // doc subcommand
    if let Some(matches) = matches.subcommand_matches("doc") {
        let name = matches.value_of("CRATE_NAME").unwrap();
        let version = matches.value_of("CRATE_VERSION");
        if let Err(e) = build_doc(name, version) {
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
        }
    } else {
        println!("{}", matches.usage());
    }
}
