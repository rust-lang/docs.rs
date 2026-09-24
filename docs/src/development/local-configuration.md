# Local Configuration

Docs.rs uses dotenv files for local configuration. Which file to edit depends on
where the application process runs:

- `.env` configures commands and Rust binaries run directly on the host. Start
  by copying `.env.sample`.
- `.docker.env` configures docs.rs processes run by Docker Compose. Start by
  copying `.docker.env.sample`, or let the `just` recipes create an empty file.

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

`.env` controls mode selection and configures commands that run on the host.
`.docker.env` configures the docs.rs process inside a Compose container; it does
not select where a recipe runs.

The GUI end-to-end CI job sets both mode variables to `docker`. It prepares
fixtures through the packaged admin and builder images before testing the
packaged web image. The ordinary local defaults favor faster host builds where
the builder supports them.

## Accessing PostgreSQL

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
