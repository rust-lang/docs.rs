# Docs.rs

[![Build Status](https://github.com/rust-lang/docs.rs/workflows/CI/badge.svg)](https://github.com/rust-lang/docs.rs/actions?workflow=CI)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://raw.githubusercontent.com/rust-lang/docs.rs/master/LICENSE)

Docs.rs (formerly cratesfyi) is an open source project to host documentation
of crates for the Rust Programming Language.

Docs.rs automatically builds crates' documentation released on crates.io using
the nightly release of the Rust compiler.

This readme is for developing docs.rs. See [the about page](https://docs.rs/about) for user-facing documentation.


## How the documentation is generated

docs.rs uses [rustdoc](https://github.com/rust-lang/rust/tree/master/src/librustdoc) to generate the documentation for every crate release on crates.io.
You can read the [the rustdoc book](https://doc.rust-lang.org/nightly/rustdoc/what-is-rustdoc.html) for more details.

## Changing the build environment

To make a change to [the build environment](https://github.com/rust-lang/crates-build-env)
and test that it works on docs.rs, see [the wiki](https://forge.rust-lang.org/docs-rs/add-dependencies.html).

## Development

The recommended way to develop docs.rs is a combination of `cargo run` for
the main binary and [docker-compose](https://docs.docker.com/compose/) for the external services.
This gives you reasonable incremental build times without having to add new users and packages to your host machine.

### Dependencies

Docs.rs requires at least the following native C dependencies.

- gcc
- g++
- pkg-config
- git
- make
- cmake
- zlib
- openssl (with dev pkgs) -- Ubuntu example `sudo apt install libssl-dev`

There may be other dependencies that have not been documented.

### Getting started

Make sure you have docker-compose and are able to download ~10GB data on the first run. Also ensure that
docker is installed and the service is running.

```sh
git clone https://github.com/rust-lang/docs.rs.git docs.rs
cd docs.rs
git submodule update --init
# Configure the default settings for external services
cp .env.sample .env
# Create the DOCSRS_PREFIX directory
mkdir -p ignored/cratesfyi-prefix/crates.io-index
# Builds the docs.rs binary
SQLX_OFFLINE=1 cargo build
# Start the external services.
docker compose up --wait db s3
# anything that doesn't run via docker-compose needs the settings defined in
# .env. Either via `. ./.env` as below, or via any dotenv shell integration.
. ./.env
# allow downloads from the s3 container to support the /crate/.../download endpoint
mcli policy set download docsrs/rust-docs-rs
# Setup the database you just created
cargo run --bin docs_rs_admin -- database migrate
# Update the currently used toolchain to the latest nightly
# This also sets up the docs.rs build environment.
# This will take a while the first time but will be cached afterwards.
cargo run --bin docs_rs_builder -- build update-toolchain
# Build a sample crate to make sure it works
cargo run --bin docs_rs_builder -- build crate regex 1.3.1
# if you don't want to run the builder, you can import a release from docs.rs itself
cargo run -p docs_rs_import_release -- regex latest
# This starts the web server but does not build any crates.
# It does not automatically run the migrations, so you need to do that manually (see above).
cargo run --bin docs_rs_web
# If you want the server to automatically restart when code or templates change
# you can use `cargo-watch`:
cargo watch -x "run --bin docs_rs_web"
```

If you need to store big files in the repository's directory it's recommended to
put them in the `ignored/` subdirectory, which is ignored both by git and
Docker.

Running the database and S3 server outside of docker-compose is possible, but not recommended or supported.
Note that you will need docker installed no matter what, since it's used for Rustwide sandboxing.

### Running tests

```
cargo test
```

To run GUI tests:

```
just run-gui-tests
```

They use the [browser-ui-test](https://github.com/GuillaumeGomez/browser-UI-test/) framework. You
can take a look at its [documentation](https://github.com/GuillaumeGomez/browser-UI-test/blob/master/goml-script.md).

Alternatively, you can start the web server and run the test manually:

```
node gui-tests/tester.js
```

For this to work, you need to install the `browser-ui-test` package:

```
npm install browser-ui-test
```

### Pure docker-compose

If you have trouble with the above commands, consider using `just compose-up-web`,
which uses docker-compose for the web server as well.
This will not cache dependencies - in particular, you'll have to rebuild all 400 whenever the lockfile changes -
but makes sure that you're in a known environment so you should have fewer problems getting started.

You can put environment overrides for the docker containers into `.docker.env`,
first. The migrations will be run by our just recipes when needed.

```sh
just cli-db-migrate
just compose-up-web
```

You can also use the `builder` compose profile to run builds on systems which don't support running builds directly (mostly on Mac OS or Windows):

```sh
just compose-up-builder

# and if needed

# update the toolchain
just cli-build-update-toolchain

# run a build for a single crate
just cli-build-crate regex 1.3.1
```

You can also run other non-build commands like the setup steps above, or queueing crates for the background builders from within the `cli` container:

```sh
just cli-db-migrate
just cli-queue-add regex 1.3.1
```

If you want to run the registry watcher, you can use the `watcher` profile:
```sh
just compose-up-watcher
```

It it was never run, we will start watching for registry changes at the current HEAD of the index.

If you want to start from another point:

```sh
just cli-queue-reset-last-seen-ref GIT_REF
```

Note that running tests is currently not supported when using pure docker-compose.

Some of the above commands are included in the `Justfile` for ease of use,
check `just --list` for an overview.

Some of the above commands are included in the `Justfile` for ease of use,
check the `[compose]` group in `just --list`.

Please file bugs for any trouble you have running docs.rs!

### Docker-Compose

The services started by Docker-Compose are defined in [docker-compose.yml].
For convenience, there are plenty of `just` recipes built around it.

[docker-compose.yml]: ./docker-compose.yml

#### Rebuilding Containers

The `just` recipes for compose handle rebuilds themselves, so nothing needs to
be done here.

If you want to completely clean up the database, don't forget to remove the volumes too:

```sh
# just shut down containers normally
$ just compose-down

# shut down and clear all volumes.
$ just compose-down-and-wipe
```

#### testing opentelemetry metrics

When you add or update any metrics you might want to test them. While there is
a way to check metric in unit-tests (see `TestEnvironment::collected_metrics`),
you might also want to test manually.

We have set up a small docker-compose service (`opentelemetry`) you can start up
via `docker compose up opentelemetry`. This start up a local instance of
the [opentelemetry collector
contrib](https://hub.docker.com/r/otel/opentelemetry-collector-contrib) image,
configured for debug-logging.

After configuring your local environment for `OTEL_EXPORTER_OTLP_ENDPOINT` => `http://localhost:4317`
(either in `.env` or `.docker.env`, depending on how you run the webserver), you
can see any metrics you report and how they are exported to your collector.

#### FAQ

##### I see the error `standard_init_linux.go:211: exec user process caused "no such file or directory"` when I use docker-compose.

You probably have [CRLF line endings](https://en.wikipedia.org/wiki/CRLF).
This causes the hashbang in the docker-entrypoint to be `/bin/sh\r` instead of `/bin/sh`.
This is probably because you have `git.autocrlf` set to true,
[set it to `input`](https://stackoverflow.com/questions/10418975) instead.

##### I see the error `/opt/rustwide/cargo-home/bin/cargo: cannot execute binary file: Exec format error` when running builds.

You are most likely not on a Linux platform. Running builds directly is only supported on `x86_64-unknown-linux-gnu`. On other platforms you can use the `docker compose run --rm builder-a build [...]` workaround described above.

See [rustwide#41](https://github.com/rust-lang/rustwide/issues/41) for more details about supporting more platforms directly.

##### All tests are failing or timing out

Our test setup needs a certain about of file descriptors.

At least 4096 should be enough, you can set it via:
```sh
$ ulimit -n 4096
```
### CLI

See `cargo run -- --help` for a full list of commands.

#### Starting the web server

```sh
# This command will start web interface of docs.rs on http://localhost:3000
cargo run --bin docs_rs_webserver start-web-server
```

#### `build` subcommand

```sh
# Builds <CRATE_NAME> <CRATE_VERSION> and adds it into database
# This is the main command to build and add a documentation into docs.rs.
# For example, `docker compose run --rm builder-a build crate regex 1.1.6`
cargo run --bin docs_rs_builder -- build crate <CRATE_NAME> <CRATE_VERSION>

# alternatively, within docker-compose containers
docker compose run --rm builder-a build crate <CRATE_NAME> <CRATE_VERSION>

# Builds every crate on crates.io and adds them into database
# (beware: this may take months to finish)
cargo run --bin docs_rs_builder -- build world

# Builds a local package you have at <SOURCE> and adds it to the database.
# The package does not have to be on crates.io.
# The package must be on the local filesystem, git urls are not allowed.
# Usually this command can be applied directly to a crate root
# In certain scenarios it might be necessary to first package the respective
# crate by using the `cargo package` command.
# See also /docs/build-workspaces.md
cargo run --bin docs_rs_builder -- build crate --local /path/to/source
```

#### `database` subcommand

```sh
# Updates repository stats for crates.
# You need to set the DOCSRS_GITHUB_ACCESSTOKEN
# environment variable in order to run this command.
# Set DOCSRS_GITLAB_ACCESSTOKEN to raise the rate limit for GitLab repositories,
# or leave it blank to fetch repositories at a slower rate.
cargo run --bin docs_rs_admin -- database update-repository-fields
```

If you want to explore or edit database manually, you can connect to the database
with the `psql` command.

```sh
. ./.env
psql $DOCSRS_DATABASE_URL
```

The database contains a blacklist of crates that should not be built.

```sh
# List the crates on the blacklist
cargo run --bin docs_rs_admin -- database blacklist list

# Adds <CRATE_NAME> to the blacklist
cargo run --bin docs_rs_admin -- database blacklist add <CRATE_NAME>

# Removes <CRATE_NAME> from the blacklist
cargo run --bin docs_rs_admin -- database blacklist remove <CRATE_NAME>
```

If you want to revert to a precise migration, you can run:

```sh
cargo run --bin docs_rs_admin -- database migrate <migration number>
```


### Updating vendored sources

The instructions & links for updating Font Awesome can be found [on their website](https://fontawesome.com/how-to-use/on-the-web/advanced/svg-sprites). Similarly, Pure-CSS also [explains on theirs](https://purecss.io/start/).

When updating Font Awesome, make sure to change `$fa-font-path` in `scss/_variables.scss` (it should be at the top of the file) to `../-/static`. This will point font awesome at the correct path from which to request font and icon resources.

### Contact

Docs.rs is run and maintained by the [docs.rs team](https://www.rust-lang.org/governance/teams/dev-tools#team-docs-rs).
You can find us in #t-docs-rs on [zulip](https://rust-lang.zulipchat.com/#narrow/stream/t-docs-rs)
