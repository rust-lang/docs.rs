
#[macro_use]
extern crate log;
extern crate rustc_serialize;
extern crate toml;
extern crate regex;
extern crate cargo;
extern crate postgres;
extern crate hyper;
extern crate time;
extern crate slug;

// Web interface dependencies
extern crate iron;
extern crate router;
extern crate handlebars_iron;
extern crate staticfile;
extern crate mount;

pub mod docbuilder;
pub mod db;
pub mod web;


/// Version string generated at build time contains last git
/// commit hash and build date
pub const BUILD_VERSION: &'static str = concat!(
    env!("CARGO_PKG_VERSION"),
    include!(concat!(env!("OUT_DIR"), "/git_version"))
);
