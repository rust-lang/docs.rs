//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.
#![deny(rust_2018_idioms)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate failure;

pub use self::docbuilder::metadata::Metadata;
pub use self::docbuilder::options::DocBuilderOptions;
pub use self::docbuilder::ChrootBuilderResult;
pub use self::docbuilder::DocBuilder;
pub use self::web::start_web_server;

pub mod db;
mod docbuilder;
pub mod error;
pub mod utils;
mod web;

/// Version string generated at build time contains last git
/// commit hash and build date
pub const BUILD_VERSION: &'static str = concat!(
    env!("CARGO_PKG_VERSION"),
    " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);
