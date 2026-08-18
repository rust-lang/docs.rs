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
