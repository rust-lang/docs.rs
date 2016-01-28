//! Crates.fyi - Documentation generator for crates released into
//! [https://crates.io](https://crates.io)
//!
//! # SYNOPSIS
//!
//! ./cratesfyi -b _package_
//!
//! # DESCRIPTION
//!
//! This script is an attempt to make a centralized documentation repository
//! for crates available in crates.io. Script is using chroot environment to
//! build documentation and fixing links on the fly.
//!
//! ## PREPARING CHROOT ENVIRONMENT
//!
//! This script is using a chroot environment to build documentation. I don't
//! think it was necessary but I didn't wanted to add bunch of stuff to my
//! stable server and a little bit more security doesn't hurt anyone.
//!
//! chroot environment must be placed in **script\_dir/chroot** directory. And
//! you must install desired version of rustc inside chroot environment. Don't
//! forget to add a regular user and create a link named **build\_home** which is
//! pointing to chroot user's home directory.  Make sure regular user is using
//! same uid with your current user. You can change username of chroot user in
//! $OPTIONS variable placed on top of this script. By default it is using
//! _onur_.
//!
//! You also need clone crates.io-index respository. You can clone repository
//! from [crates.io-index](https://github.com/rust-lang/crates.io-index).
//!
//! This script is using _sudo_ to use chroot command. chroot is only command
//! called by sudo in this script. Make sure user has rights to call chroot
//! command with sudo.
//!
//! And lastly you need to copy build.sh script into users home directory with
//! **.build.sh** name. Make sure chroot user has permissions to execute
//! **.build.sh** script.
//!
//! Directory structure should look like this:
//!
//! ```text
//! .
//! ├── cratesfyi                       # Main program
//! ├── build_home -> chroot/home/onur  # Sym link to chroot user's home
//! ├── chroot                          # chroot environment
//! │   ├── bin
//! │   ├── etc
//! │   ├── home
//! │   │   └── onur                    # chroot user's home directory
//! │   │       └── .build.sh           # Build script to run cargo doc
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
//!     Build documentation of a crate. If no crate name is provided, script will
//!     try to build documentation for all crates.
//!
//! - **-v, --version** _version_
//!
//!     Build documentation of a crate with given version. Otherwise script will
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
//!     directory. Default value: **script\_dir/public\_html/crates**
//!
//! - **--chroot** _path_
//!
//!     Chroot path. Default value: **script\_dir/chroot**
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

pub mod docbuilder;



fn main() {


    let crte = docbuilder::crte::Crate::new("sdl2".to_string(),
                                            vec!["0.9.1".to_string()]);

    let mut prefix = std::env::current_dir().unwrap();
    prefix.push("../cratesfyi-prefix");

    let sex = docbuilder::DocBuilder::from_prefix(prefix);
    println!("{:#?}", sex);

    //sex.build_doc_for_every_crate();
    let res = sex.build_doc_for_crate_version(&crte, 0);
    println!("{:#?}", res);
}
