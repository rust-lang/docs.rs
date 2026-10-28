# Local Configuration

Docs.rs uses `.env` for local configuration on the host and in Docker Compose.
Start by copying `.env.sample`. Compose overrides connection URLs and paths with
values appropriate for its internal network and mounted directories.

The `just` command loads `.env` automatically. If you invoke `cargo` directly,
source the file first or use a dotenv integration for your shell:

```console
$ . ./.env
$ cargo run --bin docs_rs_web
```

## CLI execution modes

The high-level CLI recipes can run their Rust binary on the host or through a
one-off Docker Compose service:

| Recipe           | Configuration             | Default                                    |
| ---------------- | ------------------------- | ------------------------------------------ |
| `just cli …`     | `DOCSRS_CLI_MODE`         | `local`                                    |
| `just watcher …` | `DOCSRS_CLI_MODE`         | `local`                                    |
| `just builder …` | `DOCSRS_BUILDER_CLI_MODE` | `local` on amd64 Linux; `docker` elsewhere |

The `local` mode uses `cargo run`. The `docker` mode builds and runs the
corresponding `cli`, `registry-watcher-cli`, or `builder-cli` Compose service.
In either mode, the recipes start the default PostgreSQL and S3 resources and
apply pending database migrations when necessary.

Set a mode in `.env` to make it the default for the repository, or override it
for one invocation:

```console
$ DOCSRS_BUILDER_CLI_MODE=docker just builder build crate regex 1.3.1
```

Both mode variables accept `local` or `docker`. Use `docker-run` to bypass mode
selection and name a Compose service explicitly:

```console
$ just docker-run builder-cli build crate regex 1.3.1
```

`.env` controls mode selection and configures both host and container processes.

The GUI end-to-end CI job sets both mode variables to `docker`. It prepares
fixtures through the packaged admin and builder images before testing the
packaged web image. The ordinary local defaults favor faster host builds where
the builder supports them.

## Local S3 ports

Set `DOCSRS_S3_PORT` in `.env` to change the published S3 API port (default:
`9000`), and `DOCSRS_S3_CONSOLE_PORT` to change the RustFS console port
(default: `9001`). The sample `.env` derives `S3_ENDPOINT` from the API port;
the `Justfile` generates the same endpoint when it is unset. For an existing
`.env`, update a hardcoded `S3_ENDPOINT` to match `.env.sample`, or remove it to
use the Justfile default. Explicit endpoint overrides take precedence.
Containers continue using `http://s3:9000`.

## Accessing PostgreSQL

Set `DOCSRS_POSTGRES_PORT` in `.env` to change the host port published by
Compose (default: `15432`). The sample `.env` derives `DOCSRS_DATABASE_URL` and
SQLx's `DATABASE_URL` from that port. The `Justfile` also generates these URLs
when they are unset, and exports the port to Compose. Containers continue
connecting to `db:5432` regardless of the published host port.

If you have an existing `.env` with a hardcoded database URL, update it to use
the variable references from `.env.sample`, or remove the URL settings to use
the Justfile defaults. Explicit URL overrides take precedence over generated
defaults.

After starting the local database, open a `psql` session with:

```console
$ just psql
```

This uses `DOCSRS_DATABASE_URL` from `.env`, falling back to the local
development database configured by the `Justfile`.

Running PostgreSQL and S3-compatible storage outside Docker Compose is not a
supported local-development configuration.

To invoke `psql` directly:

```console
$ . ./.env
$ psql "$DOCSRS_DATABASE_URL"
```
