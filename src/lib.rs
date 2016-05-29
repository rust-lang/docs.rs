
#[macro_use]
extern crate log;
<<<<<<< HEAD
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
=======
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
mod build_doc;
mod copy;
mod docbuilder;
>>>>>>> 0.2.0


/// Version string generated at build time contains last git
/// commit hash and build date
pub const BUILD_VERSION: &'static str = concat!(
<<<<<<< HEAD
    env!("CARGO_PKG_VERSION"),
    include!(concat!(env!("OUT_DIR"), "/git_version"))
=======
    env!("CARGO_PKG_VERSION"), " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
>>>>>>> 0.2.0
);
