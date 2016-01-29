//! Crates.fyi - Documentation generator for crates released into
//! [https://crates.io](https://crates.io)
//!
//! # SYNOPSIS
//!
//! ./cratesfyi -b _package_
//!
//! # DESCRIPTION
//!
//! This program is an attempt to make a centralized documentation repository
//! for crates available in crates.io. Program is using chroot environment to
//! build documentation and fixing links on the fly.
//!
//! ## PREPARING CHROOT ENVIRONMENT
//!
//! This program is using a chroot environment to build documentation. I don't
//! think it was necessary but I didn't wanted to add bunch of stuff to my
//! stable server and a little bit more security doesn't hurt anyone.
//!
//! chroot environment must be placed in **program\_dir/chroot** directory. And
//! you must install desired version of rustc inside chroot environment. Don't
//! forget to add a regular user and create a symbolic link named **build\_home** which is
//! pointing to chroot user's home directory.  Make sure regular user is using
//! same uid with your current user. You can change username of chroot user in
//! $OPTIONS variable placed on top of this program. By default it is using
//! _onur_.
//!
//! You also need clone crates.io-index respository. You can clone repository
//! from [crates.io-index](https://github.com/rust-lang/crates.io-index).
//!
//! This program is using _sudo_ to use chroot command. chroot is only command
//! called by sudo in this program. Make sure user has privileges to call chroot
//! command with sudo.
//!
//! And lastly you need to copy build.sh program into users home directory with
//! **.build.sh** name. Make sure chroot user has permissions to execute
//! **.build.sh** program.
//!
//! Directory structure should look like this:
//!
//! ```text
//! .
//! ├── cratesfyi                       # Main program (or cwd)
//! ├── build_home -> chroot/home/onur  # Sym link to chroot user's home
//! ├── chroot                          # chroot environment
//! │   ├── bin
//! │   ├── etc
//! │   ├── home
//! │   │   └── onur                    # chroot user's home directory
//! │   │       └── .build.sh           # Build program to run cargo doc
//! │   └── ...
//! ├── crates.io-index                 # Clone of crates.io-index
//! │   ├── 1
//! │   ├── 2
//! │   └── ...
//! ├── logs                            # Build logs will be placed here
//! │   └── ...
//! └── public_html
//!     └── crates                      # Documentations will be placed here
//! ```
//!
//!
//! # ARGS
//!
//! - **-b, --build-documentation** _crate_
//!
//!     Build documentation of a crate. If no crate name is provided, program will
//!     try to build documentation for all crates.
//!
//! - **-v, --version** _version_
//!
//!     Build documentation of a crate with given version. Otherwise program will
//!     try to build documentation for all versions. This option must be used with
//!     _-b_ argument and a crate name.
//!
//! - **-s, --skip**
//!
//!     Skip generating if documentation is exist in destination directory.
//!
//! - **--skip-tried**
//!
//!     Skips generating documentation if it's already tried before and log file is
//!     available for crate in logs directory.
//!
//! - **-k, --keep-build-directory**
//!
//!     Keep crate files in build directory after operation finishes.
//!
//! - **--destination** _path_
//!
//!     Destination path. Generated documentation directories will be moved to this
//!     directory. Default value: **program\_dir/public\_html/crates**
//!
//! - **--chroot** _path_
//!
//!     Chroot path. Default value: **program\_dir/chroot**
//!
//! - **--debug**
//!
//!     Show debug messages and place debug info in logs.
//!
//! - **-h, --help**
//!
//!     Show usage information and exit.
//!
//! # COPYRIGHT
//!
//! Copyright 2016 Onur Aslan.
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.
//!
//! This program is distributed in the hope that it will be useful,
//! but WITHOUT ANY WARRANTY; without even the implied warranty of
//! MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//! GNU General Public License for more details.
//!
//! You should have received a copy of the GNU General Public License
//! along with this program.  If not, see
//! [http://www.gnu.org/licenses/](http://www.gnu.org/licenses/).


extern crate rustc_serialize;
extern crate toml;
extern crate regex;
extern crate clap;

pub mod docbuilder;

use std::path::PathBuf;

use docbuilder::DocBuilderError;
use docbuilder::crte::Crate;
use clap::{Arg, App, SubCommand};


fn main() {

    let matches = App::new("cratesfyi")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Crate documentation builder")
        .subcommand(SubCommand::with_name("build")
                    .about("Builds documentation for a crate")
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
                         .help("Skips building documentation if documentation exists"))
                    .arg(Arg::with_name("SKIP_IF_LOG_EXISTS")
                         .long("skip-if-log-exists")
                         .help("Skips building documentation if build log exists"))
                    .arg(Arg::with_name("KEEP_BUILD_DIRECTORY")
                         .short("-k")
                         .long("keep-build-directory")
                         .help("Keeps build directory after build."))
                    .subcommand(SubCommand::with_name("world")
                                .about("Builds documentation of every crate")
                                .arg(Arg::with_name("BUILD_ONLY_LATEST_VERSION")
                                     .long("build-only-latest-version")
                                     .help("Builds only latest version of crate and \
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
                                     // This is what I got after rustfmt
                                     .get_matches();

    // DocBuilder
    if let Some(matches) = matches.subcommand_matches("build") {
        let mut dbuilder = {
            if let Some(prefix) = matches.value_of("PREFIX") {
                docbuilder::DocBuilder::from_prefix(PathBuf::from(prefix))
            } else {
                docbuilder::DocBuilder::default()
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

        // set crates.io-index path
        if let Some(crates_io_index_path) = matches.value_of("CRATES_IO_INDEX_PATH") {
            dbuilder.destination(PathBuf::from(crates_io_index_path));
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
            dbuilder.build_doc_for_every_crate();
        }

        // build single crate
        else if let Some(matches) = matches.subcommand_matches("crate") {
            // Safe to call unwrap here
            let crte_name = matches.value_of("CRATE_NAME").unwrap();
            let version = matches.value_of("CRATE_VERSION").unwrap();
            let crte = Crate::new(crte_name.to_string(), vec![version.to_string()]);

            if let Err(e) = dbuilder.build_doc_for_crate_version(&crte, 0) {
                match e {
                    DocBuilderError::SkipDocumentationExists =>
                        println!("Skipping {} documentation already exists",
                                 crte.canonical_name(0)),
                    _ => println!("Failed to build documentation for {}: {:?}",
                                  crte.canonical_name(0), e),
                }
            }
        }
    }

}
