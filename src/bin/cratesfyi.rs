
extern crate cratesfyi;
extern crate clap;
#[macro_use]
extern crate log;
extern crate time;


use std::env;
use std::fs;
use std::process::{Command, exit};
use std::path::PathBuf;

use cratesfyi::docbuilder::{DocBuilder, DocBuilderError, command_result};
use cratesfyi::docbuilder::crte::Crate;
use clap::{Arg, App, SubCommand};
use log::{LogLevel, LogLevelFilter, LogRecord, LogMetadata};
use time::now;



// I am using really simple Logger in this program
struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &LogMetadata) -> bool {
        metadata.level() <= LogLevel::Info
    }

    fn log(&self, record: &LogRecord) {
        if self.enabled(record.metadata()) {
            println!("{} {} - {}", now().to_utc().rfc3339(), record.level(), record.args());
        }
    }
}



fn update_crates_io_index(path: &PathBuf) -> Result<String, String> {
    info!("Updating crates.io-index");

    if path.exists() {
        let cwd = env::current_dir().unwrap();
        env::set_current_dir(path).unwrap();
        let res = command_result(Command::new("git").arg("pull").output().unwrap());
        env::set_current_dir(cwd).unwrap();
        res
    } else {
        command_result(Command::new("git")
            .arg("clone")
            .arg("https://github.com/rust-lang/crates.io-index.git")
            .arg(path.to_str().unwrap())
            .output().unwrap())
    }
}


// This will remove everything in CWD!!!
fn clean_build_dir() -> Result<(), DocBuilderError> {

    for file in try!(env::current_dir().unwrap()
                     .read_dir().map_err(DocBuilderError::RemoveBuildDir)) {
        let file = try!(file.map_err(DocBuilderError::RemoveBuildDir));
        let path = file.path();

        if path.file_name().unwrap() == ".cargo" ||
            path.file_name().unwrap() == ".crates.io-index" {
                continue;
            }

        if path.is_dir() {
            try!(fs::remove_dir_all(path).map_err(DocBuilderError::RemoveBuildDir));
        } else {
            try!(fs::remove_file(path).map_err(DocBuilderError::RemoveBuildDir));
        }
    }

    Ok(())
}



fn main() {
    log::set_logger(|max_log_level| {
        max_log_level.set(LogLevelFilter::Info);
        Box::new(Logger)
    }).unwrap();

    let matches = App::new("cratesfyi")
                      .version(env!("CARGO_PKG_VERSION"))
                      .about("Crates for your info!")
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
                                      .arg(Arg::with_name("LOGS_PATH")
                                               .long("logs-path")
                                               .help("Sets logs path")
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
                                      .subcommand(SubCommand::with_name("download-sources")
                                                      .about("Downloads sources of all crates"))
                                      .subcommand(SubCommand::with_name("world")
                                                      .about("Builds documentation of every \
                                                              crate")
                                                      .arg(Arg::with_name("BUILD_ONLY_LATEST_V\
                                                                           ERSION")
                                                               .long("build-only-latest-versio\
                                                                      n")
                                                               .help("Builds only latest \
                                                                      version of crate and \
                                                                      skips oldest versions")))
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
                      .subcommand(SubCommand::with_name("build-doc")
                                      .about("Builds documentation in CWD")
                                      .arg(Arg::with_name("CRATES_IO_INDEX_PATH")
                                               .short("p")
                                               .long("crates-io-index-path")
                                               .help("Sets crates.io-index path")
                                               .takes_value(true))
                                      .arg(Arg::with_name("CLEAN")
                                               .short("c")
                                               .long("clean")
                                               .help("Clean build dir before building"))
                                      .arg(Arg::with_name("CRATE_NAME")
                                               .index(1)
                                               .required(true)
                                               .help("Crate name"))
                                      .arg(Arg::with_name("CRATE_VERSION")
                                               .index(2)
                                               .required(true)
                                               .help("Version of crate")))
                      .get_matches();

    // DocBuilder
    if let Some(matches) = matches.subcommand_matches("build") {
        let mut dbuilder = {
            if let Some(prefix) = matches.value_of("PREFIX") {
                DocBuilder::from_prefix(PathBuf::from(prefix))
            } else {
                DocBuilder::default()
            }
        };

        // set destination
        if let Some(destination) = matches.value_of("DESTINATION") {
            dbuilder.destination(PathBuf::from(destination));
        }

        // set chroot path
        if let Some(chroot_path) = matches.value_of("CHROOT_PATH") {
            dbuilder.destination(PathBuf::from(chroot_path));
        }

        // set chroot user name
        if let Some(chroot_user) = matches.value_of("CHROOT_USER") {
            dbuilder.chroot_user(chroot_user.to_string());
        }

        // set crates.io-index path
        if let Some(crates_io_index_path) = matches.value_of("CRATES_IO_INDEX_PATH") {
            dbuilder.crates_io_index_path(PathBuf::from(crates_io_index_path));
        }

        // set logs path
        if let Some(logs_path) = matches.value_of("LOGS_PATH") {
            dbuilder.logs_path(PathBuf::from(logs_path));
        }

        dbuilder.skip_if_exists(matches.is_present("SKIP_IF_EXISTS"));
        dbuilder.skip_if_log_exists(matches.is_present("SKIP_IF_LOG_EXISTS"));
        dbuilder.keep_build_directory(matches.is_present("KEEP_BUILD_DIRECTORY"));

        // check paths
        if let Err(e) = dbuilder.check_paths() {
            println!("{:?}\nUse --help to get more information", e);
            std::process::exit(1);
        }

        // build world
        if let Some(matches) = matches.subcommand_matches("world") {
            dbuilder.build_only_latest_version(matches.is_present("BUILD_ONLY_LATEST_VERSION"));
            if let Err(e) = dbuilder.build_doc_for_every_crate() {
                println!("Failed to build world: {:#?}", e);
            }
        } else if let Some(matches) = matches.subcommand_matches("crate") {
            // Safe to call unwrap here
            let crte_name = matches.value_of("CRATE_NAME").unwrap();
            let version = matches.value_of("CRATE_VERSION").unwrap();
            let crte = Crate::new(crte_name.to_string(), vec![version.to_string()]);

            if let Err(e) = dbuilder.build_doc_for_crate_version(&crte, 0) {
                match e {
                    DocBuilderError::SkipDocumentationExists => {
                        println!("Skipping {} documentation already exists",
                                 crte.canonical_name(0))
                    }
                    _ => {
                        println!("Failed to build documentation for {}: {:?}",
                                 crte.canonical_name(0),
                                 e)
                    }
                }
            }
        } else if let Some(_) = matches.subcommand_matches("download-sources") {
            if let Err(e) = dbuilder.download_sources() {
                println!("{:?}", e);
            }
        }
    }


    // build-doc
    else if let Some(matches) = matches.subcommand_matches("build-doc") {
        let mut docbuilder = DocBuilder::default();
        let mut crates_io_index_path;

        // set crates.io-index path
        if let Some(crates_io_index_path_conf) = matches.value_of("CRATES_IO_INDEX_PATH") {
            crates_io_index_path = PathBuf::from(crates_io_index_path_conf);
            docbuilder.crates_io_index_path(PathBuf::from(&crates_io_index_path));
        } else {
            crates_io_index_path = env::home_dir().unwrap();
            crates_io_index_path.push(".crates.io-index");
            docbuilder.crates_io_index_path(PathBuf::from(&crates_io_index_path));
        }


        // update crates.io-index path
        if let Err(e) = update_crates_io_index(&crates_io_index_path) {
            panic!("{}", e);
        }

        // crates.io-index required for single crate
        if let Err(e) = docbuilder.check_crates_io_index_path() {
            panic!("{:?}", e);
        }

        let crte_name = matches.value_of("CRATE_NAME").unwrap();
        let version = matches.value_of("CRATE_VERSION").unwrap();

        let crte = Crate::new(crte_name.to_string(), vec![version.to_string()]);

        if matches.is_present("CLEAN") {
            clean_build_dir().unwrap();
        }

        if let Err(e) = crte.build_crate_doc(0, &docbuilder) {
            error!("Failed to build crate\n{:?}", e);
            exit(1);
        } else {
            info!("Crate successfully built!");
        }
    }

}
