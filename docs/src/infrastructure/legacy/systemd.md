# systemd Service Setup

Each docs.rs systemd service has a configuration file in dotenv format. systemd
reads these files to provide the environment in which the services run.

| service          | systemd service name | service config                          |
| ---------------- | -------------------- | --------------------------------------- |
| web server       | `docs.rs`            | `/home/cratesfyi/.docs-rs-env`          |
| index watcher    | `docs.rs.watcher`    | `/home/cratesfyi/.docs-rs-watcher-env`  |
| builder 1        | `docs.rs.builder`    | `/home/cratesfyi/.docs-rs-builder-env`  |
| builder 2        | `docs.rs.builder3`   | `/home/cratesfyi/.docs-rs-builder3-env` |
| builder 3        | `docs.rs.builder4`   | `/home/cratesfyi/.docs-rs-builder4-env` |
| builder 4        | `docs.rs.builder5`   | `/home/cratesfyi/.docs-rs-builder5-env` |
| builder 5        | `docs.rs.builder6`   | `/home/cratesfyi/.docs-rs-builder6-env` |
| docker           | `docker`             |                                         |
| nginx            | `nginx`              |                                         |
| prune-disk-space | `prune-disk-space`   |                                         |

## Web socket activation

`docs_rs_web` accepts a single TCP listening socket from systemd. When supplied,
this socket takes precedence over the web bind address. Without socket
activation, the server binds its address as usual.

For the web service `docs.rs.service`, create
`/etc/systemd/system/docs.rs.socket`:

```ini
[Unit]
Description=docs.rs HTTP socket

[Socket]
ListenStream=127.0.0.1:3000
Accept=no

[Install]
WantedBy=sockets.target
```

Add the following dependencies to the existing service's `[Unit]` section:

```ini
Requires=docs.rs.socket
After=docs.rs.socket
```

Keep the existing service command, user, and environment settings. The command
must execute `docs_rs_web` directly, or use `exec` in a shell wrapper, so that
the socket activation PID identifies the backend process.

After installing a backend version with socket activation support, switch over
with the following commands. This initial switch briefly interrupts traffic:

```bash
sudo systemctl daemon-reload
sudo systemctl stop docs.rs.service
sudo systemctl enable --now docs.rs.socket
sudo systemctl start docs.rs.service
```

For subsequent deployments, restart only `docs.rs.service`, keeping the socket
unit running. New connections can queue while the backend restarts. Existing
requests rely on graceful shutdown; systemd's service shutdown timeout and
nginx's response timeout must allow enough time. The connection backlog is
finite, so socket activation does not guarantee uninterrupted service for long
restarts or heavy traffic. nginx can continue proxying to `127.0.0.1:3000`.

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
