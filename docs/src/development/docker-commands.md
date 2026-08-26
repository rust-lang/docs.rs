# Docker Compose

The local services are defined in the repository's `docker-compose.yml`. The
`just` recipes wrap common Compose operations, run database migrations when
needed, and rebuild application images automatically when their inputs change.
Containerized application builds have less effective incremental caching than
host builds, so changes to `Cargo.lock` can require a lengthy dependency
rebuild.

List all available recipes with:

```console
$ just --list
```

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
