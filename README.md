# Docs.rs

[![Build Status](https://secure.travis-ci.org/onur/docs.rs.svg?branch=master)](https://travis-ci.org/onur/docs.rs)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://raw.githubusercontent.com/onur/docs.rs/master/LICENSE)

Docs.rs (formerly cratesfyi) is an open source project to host documentation
of crates for the Rust Programming Language.

Docs.rs automatically builds crates' documentation released on crates.io using
the nightly release of the Rust compiler.

The README of a crate is taken from the readme field defined in Cargo.toml.
If a crate doesn't have this field, no README will be displayed.

### Redirections

Docs.rs is using semver to parse URLs. You can use this feature to access
crates' documentation easily. Example of URL redirections for `clap` crate:

| URL                          | Redirects to documentation of                  |
|------------------------------|------------------------------------------------|
| <https://docs.rs/clap>       | Latest version of clap                         |
| <https://docs.rs/clap/^2>    | 2.* version                                    |
| <https://docs.rs/clap/^2.9>  | 2.9.* version                                  |
| <https://docs.rs/clap/2.9.3> | 2.9.3 version (you don't need = unlike semver) |

The crates.fyi domain will redirect to docs.rs, supporting all of the
redirects discussed above

#### Contributors

* [Onur Aslan](https://github.com/onur])
* [Corey Farwell](https://github.com/frewsxcv)
* [Jon Gjengset](https://github.com/jonhoo)
* [Matthew Hall](https://github.com/mattyhall)
* [Guillaume Gomez](https://github.com/GuillaumeGomez)
* [Mark Simulacrum](https://github.com/Mark-Simulacrum)

#### Sponsors

Hosting generously provided by:

![Leaseweb](https://docs.rs/leaseweb.gif)

If you are interested in sponsoring Docs.rs, please don't hesitate to
contact us at TODO.
