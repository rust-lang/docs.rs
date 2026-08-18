# Legacy Daemon

We still maintain a legacy daemon binary
[in the `cratesfyi` subcrate](https://github.com/rust-lang/docs.rs/tree/main/crates/bin/cratesfyi).

It's only used in the legacy infrastructure, and merges these services in one
process:

- a Web Server
- an index watcher
- one build-server
