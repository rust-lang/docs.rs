# systemd service setup

For each docs.rs systemd service we have a config file in dotenv format. Systemd
reads from them to povide the environment for running the services.

| service          | systemd service name | service config                          |
| ---------------- | -------------------- | --------------------------------------- |
| main daemon      | `docs.rs`            | `/home/cratesfyi/.docs-rs-env`          |
| second builder   | `docs.rs.builder`    | `/home/cratesfyi/.docs-rs-builder-env`  |
| third builder    | `docs.rs.builder3`   | `/home/cratesfyi/.docs-rs-builder3-env` |
| forth builder    | `docs.rs.builder4`   | `/home/cratesfyi/.docs-rs-builder3-env` |
| docker           | `docker`             |                                         |
| nginx            | `nginx`              |                                         |
| prune-disk-space | `prune-disk-space`   |                                         |

## `prune-disk-space`

Is a scheduled daily systemd task that runs some cleanup to free disk space.

What and when we run is configured in
`/etc/systemd/system/prune-disk-space.{service,timer}`.

```bash
# example, at the time of writing
docker container prune --force
docker image prune --force
cargo-sweep sweep /home/ubuntu/docs.rs --installed
```
