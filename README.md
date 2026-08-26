# Docs.rs

[![Build Status](https://github.com/rust-lang/docs.rs/workflows/CI/badge.svg)](https://github.com/rust-lang/docs.rs/actions?workflow=CI)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://raw.githubusercontent.com/rust-lang/docs.rs/main/LICENSE)

[Docs.rs](https://docs.rs/) (formerly cratesfyi) hosts documentation for crates
published on [crates.io](https://crates.io/). It builds documentation with
[rustdoc](https://github.com/rust-lang/rust/tree/main/src/librustdoc) and the
nightly Rust toolchain.

This README contains the commands needed to develop and test docs.rs. See the
[docs.rs about page](https://docs.rs/about) for user-facing documentation and
the [developer guide](https://rust-lang.github.io/docs.rs/) for architecture,
infrastructure, operations, and design documentation.

## Development

The recommended setup runs the Rust binaries on the host and external services
with Docker Compose. This provides fast incremental Rust builds without
requiring PostgreSQL or S3-compatible storage on the host.

Building crates still requires Docker because docs.rs uses `rustwide` to run
crate builds in isolated containers.

### Prerequisites

Install:

- Rust and Cargo;
- Docker with the Compose plugin;
- Git;
- GCC and G++;
- `pkg-config`;
- Make and CMake;
- zlib development files; and
- OpenSSL development files, such as `libssl-dev` on Ubuntu.

The initial setup downloads roughly 10 GB of data.

### Set up the repository

```console
$ git clone https://github.com/rust-lang/docs.rs.git docs.rs
$ cd docs.rs
$ cp .env.sample .env
$ mkdir -p ignored/cratesfyi-prefix/crates.io-index
$ SQLX_OFFLINE=1 cargo build
```

Start PostgreSQL and the local S3 service, then initialize them:

```console
$ docker compose up --wait db s3
$ . ./.env
$ cargo run --bin docs_rs_admin -- database migrate
```

Commands run outside Docker Compose need the environment variables from `.env`.
Either source it as above or use a dotenv integration for your shell.

Large local files should go in `ignored/`, which is excluded from both Git and
Docker build contexts.

### Run the web server

```console
$ . ./.env
$ cargo run --bin docs_rs_web
```

The site is available at <http://localhost:3000>. To restart it automatically
when Rust source or templates change, install `cargo-watch` and run:

```console
$ . ./.env
$ cargo watch -x "run --bin docs_rs_web"
```

### Build documentation for a crate

Set up or update the docs.rs nightly toolchain, then build a release:

```console
$ . ./.env
$ cargo run --bin docs_rs_builder -- build update-toolchain
$ cargo run --bin docs_rs_builder -- build crate regex 1.3.1
```

To test a local package instead:

```console
$ cargo run --bin docs_rs_builder -- build crate --local /path/to/package
```

Some workspace packages must first be packaged with Cargo. See
[Building workspace packages](https://rust-lang.github.io/docs.rs/development/build-workspaces.html).

If you only need an existing release in your local environment, import it
instead of running the builder:

```console
$ . ./.env
$ cargo run -p docs_rs_import_release -- regex latest
```

### Run with Docker Compose only

If running the Rust binaries on the host is impractical, the `just` recipes can
also run them in Docker Compose:

```console
$ just cli-db-migrate
$ just compose-up-web
```

Additional services can be started as needed:

```console
$ just compose-up-builder
$ just compose-up-watcher
```

Common one-off commands include:

```console
$ just cli-build-update-toolchain
$ just cli-build-crate regex 1.3.1
$ just cli-queue-add regex 1.3.1
```

Use `just --list` to see all available recipes. Tests are not currently
supported in the Docker-Compose-only development environment.

To stop the services while retaining their data, or to remove their local data:

```console
$ just compose-down
$ just compose-down-and-wipe
```

The second command removes this Compose project's containers, images, volumes,
and other local artifacts.

## Testing

Run the complete Rust workspace test suite with:

```console
$ just run-tests
```

This starts PostgreSQL and S3, builds tests for every workspace member, and runs
`cargo test --workspace --locked --no-fail-fast` with the required test
environment. Plain `cargo test` only tests the workspace's default members.

Run the ignored builder tests separately with:

```console
$ just run-builder-tests
```

Run the complete lint suite with:

```console
$ just lint
```

Run browser-based GUI tests with:

```console
$ just run-gui-tests
```

These tests use
[browser-ui-test](https://github.com/GuillaumeGomez/browser-UI-test/); its
[script documentation](https://github.com/GuillaumeGomez/browser-UI-test/blob/main/goml-script.md)
describes the test format. To run the browser test runner manually against an
already-running web server, install the package and invoke the script directly:

```console
$ npm install browser-ui-test
$ node gui-tests/tester.js
```

The test suite needs at least 4096 open file descriptors. If tests fail or time
out because the limit is too low, raise it in the current shell:

```console
$ ulimit -n 4096
```

## Developer guide

The developer guide covers the components and workflows beyond this basic setup,
including:

- [binaries and services](https://rust-lang.github.io/docs.rs/binaries/index.html);
- [development notes](https://rust-lang.github.io/docs.rs/development/index.html);
- [infrastructure](https://rust-lang.github.io/docs.rs/infrastructure/index.html);
- [production operations](https://rust-lang.github.io/docs.rs/operations/index.html);
  and
- [design documentation](https://rust-lang.github.io/docs.rs/design/index.html).

Build and open the guide locally with:

```console
$ just book-open
```

Test its examples and links with:

```console
$ just book-test
```

## Build environment

Docs.rs and `rustwide` use the
[`crates-build-env` Docker images](https://github.com/rust-lang/crates-build-env)
as the crate build environment. Add missing system dependencies there.

## Contact

Docs.rs is run and maintained by the
[docs.rs team](https://www.rust-lang.org/governance/teams/dev-tools#team-docs-rs).
You can find us in
[#t-docs-rs on Zulip](https://rust-lang.zulipchat.com/#narrow/stream/t-docs-rs).
Development problems and bugs can also be reported in the
[issue tracker](https://github.com/rust-lang/docs.rs/issues).
