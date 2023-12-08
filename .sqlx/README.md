# stored queries for sqlx offline mode

We want to have both nice local development with sqlx and a local database, and
also possible builds without having a db, like in CI or on the server.

So we need to store the queries here.

See also:

- [how to compile without a database](https://github.com/launchbadge/sqlx/blob/main/FAQ.md#how-do-i-compile-with-the-macros-without-needing-a-database-eg-in-ci)
- [`sqlx::query!` offline mode](https://docs.rs/sqlx/latest/sqlx/macro.query.html#offline-mode)
- [`cargo sqlx prepare`](https://github.com/launchbadge/sqlx/blob/main/sqlx-cli/README.md#enable-building-in-offline-mode-with-query)
