# Docs.rs

[![Build Status](https://github.com/rust-lang/docs.rs/workflows/CI/badge.svg)](https://github.com/rust-lang/docs.rs/actions?workflow=CI)
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

| URL                                           | Redirects to documentation of                    |
|-----------------------------------------------|--------------------------------------------------|
| <https://docs.rs/clap>                        | Latest version of clap                           |
| <https://docs.rs/clap/~2>                     | 2.* version                                      |
| <https://docs.rs/clap/~2.9>                   | 2.9.* version                                    |
| <https://docs.rs/clap/2.9.3>                  | 2.9.3 version (you don't need = unlike semver)   |
| <https://docs.rs/clap/*/clap/struct.App.html> | Latest version of this page (if it still exists).|

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

We strongly recommend using [docker-compose](https://docs.docker.com/compose/),
which will make it easier to get started without adding new users and packages
to your host machine.

### Getting started

Make sure you have docker-compose and are able to download ~10GB data on the first run.

```sh
git clone https://github.com/rust-lang/docs.rs.git docs.rs
cd docs.rs
cp .env.sample .env

docker-compose build  # This builds the docs.rs binary

# Build a sample crate to make sure it works
# This sets up the docs.rs build environment, including installing the nightly
# Rust toolchain. This will take a while the first time but will be cached afterwards.
docker-compose run web build crate regex 1.3.1

# This starts the web server but does not build any crates.
# If you want to build crates, see below under `build` subcommand.
# It should print a link to the website once it finishes initializing.
docker-compose up

```

If you need to store big files in the repository's directory it's recommended to
put them in the `ignored/` subdirectory, which is ignored both by git and
Docker.

### Running tests

Tests are run outside of the docker-compose environment, and can be run with:

```
cargo test
```

Some tests require access to the database. To run them, set the
`CRATESFYI_DATABASE_URL` to the url of a PostgreSQL database. You don't have to
run the migrations on it or ensure it's empty, as all the tests use temporary
tables to prevent conflicts with each other or existing data. See the [wiki
page on developing outside docker-compose][wiki-no-compose] for more
information on how to setup this environment.

[wiki-no-compose]: https://github.com/rust-lang/docs.rs/wiki/Developing-without-docker-compose

### Docker-Compose

#### Rebuilding Containers

To rebuild the site, run `docker-compose build`.
Note that docker-compose caches the build even if you change the source code,
so this will be necessary anytime you make changes.

#### FAQ

##### I keep getting the error `standard_init_linux.go:211: exec user process caused "no such file or directory"` when I use docker-compose.

You probably have [CRLF line endings](https://en.wikipedia.org/wiki/CRLF).
This causes the hashbang in the docker-entrypoint to be `/bin/sh\r` instead of `/bin/sh`.
This is probably because you have `git.autocrlf` set to true,
[set it to `input`](https://stackoverflow.com/questions/10418975) instead.

### CLI

#### Starting web server

```sh
# This command will start web interface of docs.rs and you can access it from
# http://localhost:3000/`
docker-compose up
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

# Builds a local package you have at <SOURCE> and adds it to the database.
# The package does not have to be on crates.io.
# The package must be on the local filesystem, git urls are not allowed.
docker-compose run -v "$(realpath <SOURCE>)":/build web build crate --local /build
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

If you want to explore or edit database manually, you can connect to the database
with the `psql` command.

```sh
# this will print the name of the container it starts
docker-compose run -d db
docker exec -it <the container name goes here> psql -U cratesfyi
```

The database contains a blacklist of crates that should not be built.

```sh
# List the crates on the blacklist
docker-compose run web database blacklist list

# Adds <CRATE_NAME> to the blacklist
docker-compose run web database blacklist add <CRATE_NAME>

# Removes <CRATE_NAME> from the blacklist
docker-compose run web database blacklist remove <CRATE_NAME>
```

#### `daemon` subcommand

```sh
# Run a persistent daemon which queues builds and starts a web server.
# Warning: This will try to queue hundreds of packages on crates.io, only start it
# if you have enough resources!
docker-compose run -p 3000:3000 web daemon --foreground
```

### Changing the build environment

To make a change to [the build environment](https://github.com/rust-lang/crates-build-env)
and test that it works on docs.rs, see [the wiki](https://github.com/rust-lang/docs.rs/wiki/Making-changes-to-the-build-environment).

### Contact

Docs.rs is run and maintained by [Rustdoc team](https://www.rust-lang.org/governance/teams/dev-tools#rustdoc).
You can find us in #docs-rs on [Discord](https://discord.gg/rust-lang).
