

extern crate cratesfyi;
#[macro_use]
extern crate log;
extern crate clap;
extern crate time;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, exit};

use cratesfyi::docbuilder::{DocBuilder, DocBuilderError, command_result};
use cratesfyi::docbuilder::crte::Crate;
use log::{LogLevel, LogLevelFilter, LogRecord, LogMetadata};
use clap::{Arg, App};
use time::now;


// I am using really simple Logger here
// I need to print everything to stdout for this program
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


    let matches = App::new("crate_builder")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Crate documentation builder")
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
             .help("Version of crate"))
        .get_matches();


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
