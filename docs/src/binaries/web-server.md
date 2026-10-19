# Web Server

Our web server:

- lives
  [in the `docs_rs_web` subcrate](https://github.com/rust-lang/docs.rs/tree/main/crates/bin/docs_rs_web),
  and
- is based on [the `axum` crate](https://docs.rs/axum/latest/axum/).

Besides serving some static and database-backed content, it acts as a proxy for
the stored rustdoc HTML files, rewriting them on the fly to match our UI.

Because we recompress and rewrite HTML for many requests, we're more CPU-bound
than a typical web server.

## Health check

`GET /-/health` (or `HEAD /-/health`) returns `200 OK` with
`Cache-Control: no-cache, no-store, must-revalidate, max-age=0` once the web
server is listening. It checks the web process only and does not query
PostgreSQL, S3, or other external services. Docker Compose uses this endpoint;
load balancers can use it on the configured web port. Monitor user-facing pages
separately to check dependency health.
