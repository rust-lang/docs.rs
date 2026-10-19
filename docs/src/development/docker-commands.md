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

Every runtime stage runs its packaged binary with `--help` during the build,
including `tini` and the runtime dependencies. A failed check fails the build
before that image is exported or published. Successful checks are cached until
an earlier image layer changes.

The testing recipe also builds and checks the complete default group:

```console
$ just docker-test
```

The images are loaded into the local Docker daemon. To repeat a runtime check
manually, for example:

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

## Run release commands

The CLI image includes `/usr/local/bin/docs_rs_release.sh`. Add commands that
should run once after each deployment to
`crates/bin/docs_rs_admin/bin/docs_rs_release.sh`.

The script applies database migrations, then purges the `docs-rs-static` CDN
key. It skips the purge unless both `DOCSRS_FASTLY_API_TOKEN` and
`DOCSRS_FASTLY_SERVICE_SID_WEB` are configured. Its `set -euo pipefail` stops
execution at the first failed command, so a migration failure also skips the
purge.

### Run locally

The image's default entrypoint runs `docs_rs_admin`. Override it to execute the
release script instead, using the same database and CDN configuration as the
admin CLI:

```console
$ docker run --rm --env-file .docker.env \
    --entrypoint /usr/bin/tini docs-rs-admin:ci -- /usr/local/bin/docs_rs_release.sh
```

### Run during ECS deployment

The
[Docker pipeline](https://github.com/rust-lang/docs.rs/blob/main/.github/workflows/docker-pipeline.yml)
publishes `docs-rs-web` and `docs-rs-admin` images with commit and `latest`
tags, then:

1. Deploys the web service using `latest` and waits for it to become stable.
2. Downloads the latest active `docs-rs-release` task definition, managed by
   Terraform.
3. Uses `aws-actions/amazon-ecs-deploy-task-definition` to register a new
   release revision, run one task, and wait for completion. Launch errors, wait
   failures, and nonzero or missing container exit codes fail the CI job.

Terraform remains the source for the release task's roles, secrets, environment
variables, resource settings, entrypoint, and command. Its image must reference
`docs-rs-admin:latest` in ECR, with the `app` container's `entryPoint` set to
`["/usr/bin/tini", "--"]` and `command` set to
`["/usr/local/bin/docs_rs_release.sh"]`. The workflow uses this configuration
without modifying it. The separate `docs-rs-admin` task definition retains its
normal admin-binary entrypoint for other one-off commands.

The release task reuses the web service's subnets, security groups, and public
IP setting so it has the same network access to the database. These settings
belong to the task launch request, rather than the task definition.

The CI role needs permission to describe services and task definitions, register
task definitions, run and describe tasks, and pass the task's IAM roles.
Permissions for running tasks must cover the `docs-rs-release` family.

The pipeline serializes deployments per environment. Keep publishing and release
execution serialized for each registry so another build cannot replace `latest`
before the release task starts.

The new web version must work with the old database schema until migrations
finish. If the release task fails, CI fails but the new web version remains
deployed; the workflow does not roll it back.

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
