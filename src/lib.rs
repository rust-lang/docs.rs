
#[macro_use]
extern crate log;
extern crate cargo;
extern crate regex;
extern crate rustc_serialize;
extern crate postgres;
extern crate hyper;
extern crate time;
extern crate semver;
extern crate slug;

pub use self::build_doc::{build_doc, get_package, source_path, update_sources};
pub use self::copy::{copy_dir, copy_doc_dir};
pub use self::docbuilder::DocBuilder;
pub use self::docbuilder::ChrootBuilderResult;
pub use self::docbuilder::error::DocBuilderError;
pub use self::docbuilder::options::DocBuilderOptions;

pub mod db;
pub mod utils;
mod build_doc;
mod copy;
mod docbuilder;


/// Version string generated at build time contains last git
/// commit hash and build date
pub const BUILD_VERSION: &'static str = concat!(
    env!("CARGO_PKG_VERSION"), " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);
