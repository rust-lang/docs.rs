# Docs.rs

[![Build Status](https://dev.azure.com/docsrs/docs.rs/_apis/build/status/docs.rs?branchName=master)](https://dev.azure.com/docsrs/docs.rs/_build/latest?definitionId=1)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://raw.githubusercontent.com/rust-lang/docs.rs/master/LICENSE)

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
| <https://docs.rs/clap/~2>    | 2.* version                                    |
| <https://docs.rs/clap/~2.9>  | 2.9.* version                                  |
| <https://docs.rs/clap/2.9.3> | 2.9.3 version (you don't need = unlike semver) |

The crates.fyi domain will redirect to docs.rs, supporting all of the
redirects discussed above


### Badges

You can use badges to show state of your documentation to your users.
The default badge will be pointed at the latest version of a crate.
You can use `version` parameter to show status of documentation for
any version you want.

Badge will display in blue if docs.rs is successfully hosting your crate
documentation, and red if building documentation failing.

Example badges for `mio` crate:

| URL   | Badge |
|-------|-------|
| Latest version: <https://docs.rs/mio/badge.svg> | ![mio](https://docs.rs/mio/badge.svg) |
| Version 0.4.4: <https://docs.rs/mio/badge.svg?version=0.4.4> | ![mio](https://docs.rs/mio/badge.svg?version=0.4.4) |
| Version 0.1.0: <https://docs.rs/mio/badge.svg?version=0.1.0> | ![mio](https://docs.rs/mio/badge.svg?version=0.1.0) |


## Development

We strongly recommend using vagrant, this will give you a virtual machine
already configured and ready to start developing on.

### Getting started

Make sure you have vagrant, virtualbox and a ssh client and you need
to able to download ~800MB data on the first run.


```sh
git clone https://github.com/rust-lang/docs.rs.git docs.rs
cd docs.rs
vagrant up  # This may take a little while on the first run
```

You can always run `vagrant provision` to reconfigure virtual machine.
Provision will install required dependencies and nightly rust compiler
into virtual machine. It will also configure lxc-container inside
virtual machine.

### CLI

Make sure you are running every listed command inside `/vagrant` directory
in virtual machine. You can connect to virtual machine with `vagrant ssh` and
switch current working directory with: `cd /vagrant` inside virtual machine.


#### Starting web server

This command will start web interface of docs.rs and you can access it from:
`http://localhost:3000/`

```
cargo run -- start-web-server
```


#### `build` subcommand

```sh
# Builds <CRATE_NAME> <CRATE_VERSION> and adds it into database
# This is the main command to build and add a documentation into docs.rs.
cargo run -- build crate <CRATE_NAME> <CRATE_VERSION>


# Adds essential files (css and fonts) into database to avoid duplication
# This command needs to be run after each rustc update
cargo run -- build add-essential-files


# Builds every crate and adds them into database
# (beware: this may take months to finish)
cargo run -- build world
```


#### `database` subcommand

```sh
# Initializes database. Currently, only creates tables in database.
cargo run -- database init


# Adds a directory into database to serve with `staticfile` crate.
cargo run -- database add-directory <DIRECTORY> [PREFIX]


# Updates github stats for crates.
# You need to set CRATESFYI_GITHUB_USERNAME, CRATESFYI_GITHUB_ACCESSTOKEN
# environment variables in order to run this command.
# You can set this environment variables in ~/.cratesfyi.env file.
cargo run -- database update-github-fields


# Updates search-index.
# daemon is running this command occasionally, and this command must be
# run to update recent-version of a crate index and search index.
# If you are having any trouble with accessing right version of a crate,
# run this command. Otherwise it's not required.
cargo run -- database update-search-index


# Updates release activitiy chart
cargo run -- database update-release-activity    
```

If you want to explore or edit database manually, you can connect database
with `psql` command.


#### `doc` subcommand

This subcommand will only build documentation of a crate.
It is designed to run inside a secure container.

```
cargo run -- doc <CRATE_NAME>
```


#### Contributors

* [Onur Aslan](https://github.com/onur)
* [Jon Gjengset](https://github.com/jonhoo)
* [Sebastian Thiel](https://github.com/Byron)
* [Guillaume Gomez](https://github.com/GuillaumeGomez)
* [Ashe Connor](https://github.com/kivikakk)
* [Samuel Tardieu](https://github.com/samueltardieu)
* [Corey Farwell](https://github.com/frewsxcv)
* [Michael Howell](https://github.com/notriddle)
* [Alex Burka](https://github.com/durka)
* [Giang Nguyen](https://github.com/hngnaig)
* [Dimitri Sabadie](https://github.com/phaazon)
* [Nemikolh](https://github.com/Nemikolh)
* [bluss](https://github.com/bluss)
* [Pascal Hartig](https://github.com/passy)
* [Matthew Hall](https://github.com/mattyhall)
* [Mark Simulacrum](https://github.com/Mark-Simulacrum)

#### Sponsors

Hosting generously provided by:

![Leaseweb](https://docs.rs/leaseweb.gif)

If you are interested in sponsoring Docs.rs, please don't hesitate to
contact us at TODO.
