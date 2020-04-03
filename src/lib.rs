//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.
#![allow(clippy::cognitive_complexity)]

#[macro_use]
extern crate log;

pub use self::docbuilder::RustwideBuilder;
pub use self::docbuilder::DocBuilder;
pub use self::docbuilder::options::DocBuilderOptions;
pub use self::web::Server;

mod error;
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
pub const BUILD_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"),
                                                " ",
                                                include_str!(concat!(env!("OUT_DIR"),
                                                                     "/git_version")));
