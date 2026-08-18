# Build Server

The build servers:

- read releases from the build queue,
- use [`rustwide`](https://docs.rs/rustwide/latest/rustwide/) to run
  `cargo doc`, isolating each build in a Docker container for security, and
- package the documentation into a ZIP file and upload it to S3.

The code lives
[in the `docs_rs_builder` subcrate](https://github.com/rust-lang/docs.rs/tree/main/crates/bin/docs_rs_builder).
