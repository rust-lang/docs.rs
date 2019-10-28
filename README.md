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

We strongly recommend using docker-compose, which will make it easier to get started
without adding new users and packages to your host machine.

### Getting started

Make sure you have docker-compose and are able to download ~10GB data on the first run.


```sh
git clone https://github.com/rust-lang/docs.rs.git docs.rs
cd docs.rs
docker-compose up  # This may take a half hour or more on the first run
```

### CLI

#### Starting web server

```sh
# This command will start web interface of docs.rs and you can access it from
# http://localhost:3000/`
docker-compose run web -p 3000:3000 start-web-server
```

#### `build` subcommand

```sh
# Builds <CRATE_NAME> <CRATE_VERSION> and adds it into database
# This is the main command to build and add a documentation into docs.rs.
# For example, `docker-compose run web build crate regex 1.1.6`
docker-compose run web build crate <CRATE_NAME> <CRATE_VERSION>

# Builds every crate and adds them into database
# (beware: this may take months to finish)
docker-compose run web build world
```


#### `database` subcommand

```sh
# Adds a directory into database to serve with `staticfile` crate.
docker-compose run web database add-directory <DIRECTORY> [PREFIX]

# Updates github stats for crates.
# You need to set CRATESFYI_GITHUB_USERNAME, CRATESFYI_GITHUB_ACCESSTOKEN
# environment variables in order to run this command.
# You can set this environment variables in ~/.cratesfyi.env file.
docker-compose run web database update-github-fields
```

If you want to explore or edit database manually, you can connect database
with `psql` command.

```sh
# this will print the name of the container it starts
docker-compose run -d db
docker exec -it <the container name goes here> psql -U cratesfyi
```

#### Contact

Docs.rs is run and maintained by [Rustdoc team](https://www.rust-lang.org/governance/teams/dev-tools#rustdoc).
You can find us in #docs-rs on [Discord](https://discord.gg/rust-lang).
