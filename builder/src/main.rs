use std::env::current_dir;
use std::path::PathBuf;

use clap::{App, Arg};
use docsrs_builder::build_doc;
use env_logger;

fn main() {
    let _ = env_logger::init();
    let matches = App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .arg(Arg::with_name("NAME").required(true).help("Crate name"))
        .arg(
            Arg::with_name("VERSION")
                .required(true)
                .help("Crate version"),
        )
        .arg(
            Arg::with_name("TARGET_DIR")
                .short("d")
                .long("directory")
                .takes_value(true)
                .help("Target directory"),
        )
        .get_matches();

    let name = matches.value_of("NAME").expect("Unable to find name");
    let version = matches.value_of("VERSION").expect("Unable to find version");

    // get target directory from command line option or use "$CWD/docsrs"
    let target_dir = matches
        .value_of("TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            current_dir()
                .expect("Unable to get current working directory")
                .join("docsrs")
        });

    build_doc(&name, &version, &target_dir).expect("Failed to build docsrs documentation package");
}
