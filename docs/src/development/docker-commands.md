# Docker Compose

The local services are defined in the repository's `docker-compose.yml`. The
`just` recipes wrap common Compose operations, run database migrations when
needed, and rebuild application images automatically when their inputs change.

List all available recipes with:

```console
$ just --list
```

## Build images directly

Docker images are defined in `docker-bake.hcl`. Build one image with its Bake
target:

```console
$ just docker-build build-server
```

Build all application images with:

```console
$ just docker-build
```

After building, run the same packaged-binary smoke tests as CI with:

```console
$ just docker-smoke-test
```

Or build and smoke-test the complete default group in one command:

```console
$ just docker-test
```

The images are loaded into the local Docker daemon. For example, smoke-test the
build-server image with:

```console
$ docker run --rm docs-rs-build-server:ci --help
```

The Docker build stores cargo-chef's compiled dependency baseline in an image
layer that can be reused by remote builders. Application builds additionally
share a Cargo target cache local to the Buildx builder, which enables
incremental rebuilds after source changes. A new Buildx builder starts without
that local incremental cache.

## Run one-off CLI commands

The `cli`, `watcher`, and `builder` recipes select host or Compose execution
through their configured CLI mode. See
[CLI execution modes](local-configuration.md#cli-execution-modes) for the
defaults, environment variables, and configuration-file behavior.

Override a mode for one command when you want to keep using its high-level
recipe:

```console
$ DOCSRS_CLI_MODE=docker just cli-db-migrate
$ DOCSRS_CLI_MODE=docker just watcher queue set-last-seen-reference --head
$ DOCSRS_BUILDER_CLI_MODE=docker just builder build crate regex 1.3.1
```

Or bypass mode selection and run a specific Compose service directly:

```console
$ just docker-run builder-cli build crate regex 1.3.1
$ just docker-run registry-watcher-cli queue set-last-seen-reference --head
```

## Start services

Start the default PostgreSQL and S3 resources used by host-side development:

```console
$ just compose-up-resources
```

Recipes that need these resources start them automatically. The explicit command
is useful before running application binaries directly with Cargo.

Start individual application profiles with:

```console
$ just compose-up-web
$ just compose-up-builder
$ just compose-up-watcher
$ just compose-up-metrics
```

Or start the complete local stack:

```console
$ just compose-up-full
```

## View logs

Follow logs for one or more services with:

```console
$ just compose-logs db
$ just compose-logs web builder-a
```

The equivalent direct Compose command is:

```console
$ docker compose logs --follow db
```

## Stop or reset services

Stop services while keeping their volumes and other local data:

```console
$ just compose-down
```

Remove this Compose project's volumes, locally built images, and other local
artifacts as well:

```console
$ just compose-down-and-wipe
```

The second command deletes the local development database and object-storage
contents.

## GUI tests

Build the crate fixtures required by the GUI suite:

```console
$ just prepare-gui-tests
```

This uses the `builder` recipe and therefore follows `DOCSRS_BUILDER_CLI_MODE`.
The generated documentation is retained in the local PostgreSQL and S3
resources.

After preparing fixtures, start a temporary host web server and run the browser
assertions with:

```console
$ just run-gui-tests
```

This does not rebuild fixtures, making it the fast path for changes to
templates, CSS, JavaScript, and web behavior. `just compose-down` preserves the
fixtures; `just compose-down-and-wipe` removes them.

If a suitable web server is already listening on port 3000, run only the browser
assertions with:

```console
$ just run-gui-browser-tests
```

To reproduce the container integration path used in CI, run:

```console
$ DOCSRS_CLI_MODE=docker DOCSRS_BUILDER_CLI_MODE=docker \
    just prepare-gui-tests run-gui-tests-e2e
```

This applies migrations through the packaged admin image, builds fixtures
through the packaged build-server image, and serves the results from the
packaged web-server image. Node and Puppeteer remain on the host so the test
driver does not require a separate image.
