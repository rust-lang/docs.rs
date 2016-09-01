//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.

#[macro_use]
extern crate log;
#[macro_use]
extern crate error_chain;
extern crate cargo;
extern crate regex;
extern crate rustc_serialize;
extern crate postgres;
extern crate hyper;
extern crate time;
extern crate semver;
extern crate slug;
extern crate git2;
extern crate magic;
extern crate iron;
extern crate router;
extern crate staticfile;
extern crate handlebars_iron;
extern crate hoedown;
extern crate r2d2;
extern crate r2d2_postgres;
extern crate url;
extern crate params;
extern crate libc;
extern crate badge;

pub use self::docbuilder::DocBuilder;
pub use self::docbuilder::ChrootBuilderResult;
pub use self::docbuilder::options::DocBuilderOptions;
pub use self::web::start_web_server;

pub mod errors;
pub mod db;
pub mod utils;
mod docbuilder;
mod web;


/// Version string generated at build time contains last git
/// commit hash and build date
pub const BUILD_VERSION: &'static str = concat!(
    env!("CARGO_PKG_VERSION"), " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);
