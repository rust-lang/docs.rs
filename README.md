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
- [mise](https://mise.jdx.dev/);
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
$ mise install
$ SQLX_OFFLINE=1 cargo build
```

`mise install` installs the project’s `just`, Node, Deno, and Cargo development
tools. Enable mise in your shell to make them available automatically, or run
commands through `mise x -- <command>`.

Start PostgreSQL and the local S3 service, then initialize the database:

```console
$ just compose-up-resources
$ just sqlx-migrate-run
```

Most recipes start these resources automatically; use
`just compose-up-resources` when running application commands directly. The
`cli`, `builder`, `watcher`, and Compose application recipes also apply pending
migrations before starting.

The `just` recipes load `.env` and start PostgreSQL and S3 when needed. Commands
run directly with Cargo need the same environment variables; source `.env` or
use a dotenv integration for your shell before running them.

Large local files should go in `ignored/`, which is excluded from both Git and
Docker build contexts.

### Run the web server

```console
$ just web
```

The site is available at <http://localhost:3000>. To restart it automatically
when web, template, asset, shared-library, or workspace configuration files
change, run:

```console
$ just web-watch
```

The watch command runs from the repository root and ignores changes confined to
other application binaries, such as the builder and registry watcher.

### Build documentation for a crate

Set up or update the docs.rs nightly toolchain, then build a release:

```console
$ just builder build update-toolchain
$ just builder build crate regex 1.3.1
```

The `builder` recipe uses `DOCSRS_BUILDER_CLI_MODE`: it defaults to `local` on
amd64 Linux and `docker` on other platforms. Set the variable explicitly to
override that choice.

The `cli` and `watcher` recipes similarly use `DOCSRS_CLI_MODE`, but default to
`local` on every platform. Set either mode variable to `docker` to keep using
the same high-level recipe through its corresponding Compose service.

To test a local package instead:

```console
$ just builder build crate --local /path/to/package
```

Some workspace packages must first be packaged with Cargo. See
[Building workspace packages](https://rust-lang.github.io/docs.rs/development/build-workspaces.html).

If you only need an existing release in your local environment, import it
instead of running the builder:

```console
$ just import-release regex latest
```

### Run with Docker Compose only

If running the Rust binaries on the host is impractical, the `just` recipes can
keep the same interface while running them through Docker Compose. Add these
settings to `.env`:

```dotenv
DOCSRS_CLI_MODE=docker
DOCSRS_BUILDER_CLI_MODE=docker
```

Then use the normal recipes:

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
$ just builder build update-toolchain
$ just builder build crate regex 1.3.1
$ just cli queue add regex 1.3.1
```

Use `just --list` to see all available recipes. The Rust test suite still runs
on the host; the GUI suite has a container integration mode described below. The
lower-level `docker-run` recipe is available when a specific Compose service
must be selected explicitly, but is not needed for normal development.

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

Test database migrations through both SQLx CLI and `docs_rs_admin` with:

```console
$ just test-database-migrations
```

After changing queries or migrations, apply migrations and regenerate the
committed SQLx offline metadata with:

```console
$ just sqlx-update
```

Run the complete lint suite with:

```console
$ just lint
```

`mise install` provides [`actionlint`](https://github.com/rhysd/actionlint),
which `just lint` uses to validate GitHub Actions workflows.

Run all formatters with:

```console
$ just format
```

If files are not formatted correctly, this command rewrites them and exits with
an error so that you can review the changes.

Prepare the GUI fixture crates once with:

```console
$ just prepare-gui-tests
```

The builder follows `DOCSRS_BUILDER_CLI_MODE`, so it normally runs on the host
on amd64 Linux and through the packaged builder image elsewhere. The generated
fixture data remains in PostgreSQL and S3.

Run the GUI tests against a temporary host web server with:

```console
$ just run-gui-tests
```

This reuses the existing fixtures, so changes limited to templates, CSS,
JavaScript, or web behavior do not require another preparation step. Fixture
data remains available after `just compose-down`, while
`just compose-down-and-wipe` removes it.

To reproduce the container integration setup used in CI, run:

```console
$ DOCSRS_CLI_MODE=docker DOCSRS_BUILDER_CLI_MODE=docker \
    just prepare-gui-tests run-gui-tests-e2e
```

This applies migrations through the packaged admin image, builds fixtures
through the packaged builder image, and serves them with the packaged web image.
The Node/Puppeteer browser runner remains on the host.

These tests use
[browser-ui-test](https://github.com/GuillaumeGomez/browser-UI-test/); its
[script documentation](https://github.com/GuillaumeGomez/browser-UI-test/blob/main/goml-script.md)
describes the test format. To run only the browser assertions against a web
server already listening on port 3000, use:

```console
$ just run-gui-browser-tests
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
