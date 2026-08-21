# systemd Service Setup

Each docs.rs systemd service has a configuration file in dotenv format. systemd
reads these files to provide the environment in which the services run.

| service          | systemd service name | service config                          |
| ---------------- | -------------------- | --------------------------------------- |
| main daemon      | `docs.rs`            | `/home/cratesfyi/.docs-rs-env`          |
| second builder   | `docs.rs.builder`    | `/home/cratesfyi/.docs-rs-builder-env`  |
| third builder    | `docs.rs.builder3`   | `/home/cratesfyi/.docs-rs-builder3-env` |
| fourth builder   | `docs.rs.builder4`   | `/home/cratesfyi/.docs-rs-builder4-env` |
| docker           | `docker`             |                                         |
| nginx            | `nginx`              |                                         |
| prune-disk-space | `prune-disk-space`   |                                         |

## `prune-disk-space`

This scheduled daily systemd task performs cleanup to free disk space.

The cleanup commands and schedule are configured in
`/etc/systemd/system/prune-disk-space.{service,timer}`.

```bash
# example, at the time of writing
docker container prune --force
docker image prune --force
cargo-sweep sweep /home/ubuntu/docs.rs --installed
```
