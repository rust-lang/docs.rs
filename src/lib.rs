//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.

#[macro_use]
extern crate log;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate prometheus;
#[macro_use]
extern crate lazy_static;
extern crate regex;
extern crate rustc_serialize;
extern crate postgres;
extern crate reqwest;
extern crate time;
extern crate semver;
extern crate slug;
extern crate mime_guess;
extern crate iron;
extern crate router;
extern crate staticfile;
extern crate handlebars_iron;
extern crate comrak;
extern crate r2d2;
extern crate r2d2_postgres;
extern crate url;
extern crate params;
extern crate libc;
extern crate badge;
extern crate crates_index_diff;
extern crate toml;
extern crate html5ever;
extern crate schemamama;
extern crate schemamama_postgres;
extern crate rusoto_s3;
extern crate rusoto_core;
extern crate rusoto_credential;
extern crate futures;
extern crate tokio;
extern crate systemstat;
extern crate rustwide;
extern crate tempdir;
#[cfg(test)]
extern crate once_cell;

pub use self::docbuilder::RustwideBuilder;
pub use self::docbuilder::DocBuilder;
pub use self::docbuilder::options::DocBuilderOptions;
pub use self::docbuilder::metadata::Metadata;
pub use self::web::Server;

pub mod error;
pub mod db;
pub mod utils;
mod docbuilder;
mod web;
#[cfg(test)]
mod test;

use web::page::GlobalAlert;


// Warning message shown in the navigation bar of every page. Set to `None` to hide it.
pub(crate) static GLOBAL_ALERT: Option<GlobalAlert> = None;
/*
pub(crate) static GLOBAL_ALERT: Option<GlobalAlert> = Some(GlobalAlert {
    url: "https://blog.rust-lang.org/2019/09/18/upcoming-docsrs-changes.html",
    text: "Upcoming docs.rs breaking changes!",
    css_class: "error",
    fa_icon: "warning",
});
*/


/// Version string generated at build time contains last git
/// commit hash and build date
pub const BUILD_VERSION: &'static str = concat!(env!("CARGO_PKG_VERSION"),
                                                " ",
                                                include_str!(concat!(env!("OUT_DIR"),
                                                                     "/git_version")));
