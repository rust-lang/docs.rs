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
