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
$ docker buildx bake build-server
```

Build all application images with:

```console
$ docker buildx bake
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

## Start services

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
