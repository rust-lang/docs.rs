# Legacy Daemon

We still maintain a legacy daemon binary
[in the `cratesfyi` subcrate](https://github.com/rust-lang/docs.rs/tree/main/crates/bin/cratesfyi).

It's used only in the legacy infrastructure and combines these services into
one process:

- a web server,
- an index watcher, and
- one build server.
