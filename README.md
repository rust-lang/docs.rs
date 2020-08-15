# Docs.rs

[![Build Status](https://github.com/rust-lang/docs.rs/workflows/CI/badge.svg)](https://github.com/rust-lang/docs.rs/actions?workflow=CI)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://raw.githubusercontent.com/rust-lang/docs.rs/master/LICENSE)

Docs.rs (formerly cratesfyi) is an open source project to host documentation
of crates for the Rust Programming Language.

Docs.rs automatically builds crates' documentation released on crates.io using
the nightly release of the Rust compiler.

This readme is for developing docs.rs. See [the about page](https://docs.rs/about) for user-facing documentation.

## Changing the build environment

To make a change to [the build environment](https://github.com/rust-lang/crates-build-env)
and test that it works on docs.rs, see [the wiki](https://forge.rust-lang.org/docs-rs/add-dependencies.html).

## Development

The recommended way to develop docs.rs is a combination of `cargo run` for
the main binary and [docker-compose](https://docs.docker.com/compose/) for the external services.
This gives you reasonable incremental build times without having to add new users and packages to your host machine.

### Git Hooks

For ease of use, `git_hooks` directory contains useful `git hooks` to make your development easier.

```bash
# Unix
cd .git/hooks && ln -s ../../.git_hooks/* . && cd ../..
# Powershell
cd .git/hooks && New-Item -Path ../../.git_hooks/* -ItemType SymbolicLink -Value . && cd ../..
```

### Dependencies

Docs.rs requires at least the following native C dependencies.

- gcc
- g++
- pkg-config
- git
- make
- cmake
- zlib
- openssl

There may be other dependencies that have not been documented.

### Getting started

Make sure you have docker-compose and are able to download ~10GB data on the first run.

```sh
git clone https://github.com/rust-lang/docs.rs.git docs.rs
cd docs.rs
# Configure the default settings for external services
cp .env.sample .env
# Builds the docs.rs binary
cargo build
# Start the extenal services
docker-compose up -d db s3
# Build a sample crate to make sure it works
# This sets up the docs.rs build environment, including installing the nightly
# Rust toolchain. This will take a while the first time but will be cached afterwards.
cargo run web build crate regex 1.3.1
# This starts the web server but does not build any crates.
cargo run start-web-server
```

If you need to store big files in the repository's directory it's recommended to
put them in the `ignored/` subdirectory, which is ignored both by git and
Docker.

### Pure docker-compose

If you have trouble with the above commands, consider using `docker-compose up`,
which uses docker-compose for the web server as well.
This will not cache dependencies as well - in particular, you'll have to rebuild all 400 whenever the lockfile changes -
but makes sure that you're in a known environment so you should have fewer problems getting started.

Please file bugs for any trouble you have running docs.rs!

### Running tests

Tests are only supported via cargo and do not work in docker-compose

```
cargo test
```

Most tests require access to the database. To run them, set the
`CRATESFYI_DATABASE_URL` in `.env` to the url of a PostgreSQL database,
and set the `AWS_ACCESS_KEY_ID`, `S3_ENDPOINT`, and `AWS_SECRET_ACCESS_KEY` variables.
We have some reasonable default parameters in `.env.sample`.

For example, if you are using the `docker-compose` environment to run tests against, you can launch only the database and s3 server like so:

```console
docker-compose up -d db s3
```

If you don't want to use docker-compose, see the
[wiki page on developing outside docker-compose][wiki-no-compose]
for more information on how to setup this environment.
Note that either way, you will need docker installed for sandboxing with Rustwide.

[wiki-no-compose]: https://forge.rust-lang.org/docs-rs/no-docker-compose.html

### Docker-Compose

The services started by Docker-Compose are defined in [docker-compose.yml].
Three services are defined:

| name | access                                          | credentials                | description                            |
|------|-------------------------------------------------|----------------------------|----------------------------------------|
| web  | http://localhost:3000                           | N/A                        | A container running the docs.rs binary |
| db   | postgresql://cratesfyi:password@localhost:15432 | -                          | Postgres database used by web          |
| s3   | http://localhost:9000                           | `cratesfyi` - `secret_key` | Minio (simulates AWS S3) used by web   |

[docker-compose.yml]: ./docker-compose.yml

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

See `cargo run -- --help` for a full list of commands.

#### Starting the web server

```sh
# This command will start web interface of docs.rs on http://localhost:3000
cargo run -- start-web-server
```

#### `build` subcommand

```sh
# Builds <CRATE_NAME> <CRATE_VERSION> and adds it into database
# This is the main command to build and add a documentation into docs.rs.
# For example, `docker-compose run web build crate regex 1.1.6`
cargo run -- build crate <CRATE_NAME> <CRATE_VERSION>

# Builds every crate on crates.io and adds them into database
# (beware: this may take months to finish)
cargo run -- build world

# Builds a local package you have at <SOURCE> and adds it to the database.
# The package does not have to be on crates.io.
# The package must be on the local filesystem, git urls are not allowed.
cargo run -- build crate --local /path/to/source
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
. .env
psql $CRATESFYI_DATABASE_URL
```

The database contains a blacklist of crates that should not be built.

```sh
# List the crates on the blacklist
cargo run -- database blacklist list

# Adds <CRATE_NAME> to the blacklist
cargo run -- database blacklist add <CRATE_NAME>

# Removes <CRATE_NAME> from the blacklist
cargo run -- database blacklist remove <CRATE_NAME>
```

#### `daemon` subcommand

```sh
# Run a persistent daemon which queues builds and starts a web server.
cargo run -- daemon --registry-watcher=disabled
# Add crates to the queue
cargo run -- queue add <CRATE> <VERSION>
```

### Contact

Docs.rs is run and maintained by the [docs.rs team](https://www.rust-lang.org/governance/teams/dev-tools#docs-rs).
You can find us in #docs-rs on [Discord](https://discord.gg/f7mTXPW).
