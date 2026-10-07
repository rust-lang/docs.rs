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

## Web socket activation

Both `docs_rs_web` and the legacy `cratesfyi daemon` accept a single TCP listening
socket from systemd. When supplied, this socket takes precedence over the web
bind address. Without socket activation, the server binds its address as usual.

For the legacy `docs.rs.service`, create `/etc/systemd/system/docs.rs.socket`:

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

For the standalone web service, use its service name for the socket unit instead.
Keep the existing service command, user, and environment settings. The command
must execute the backend directly, or use `exec` in a shell wrapper, so that the
socket activation PID identifies the backend process.

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

### Testing the listener

The listener tests bind loopback sockets and do not require a database or a full
webserver context:

```bash
SQLX_OFFLINE=true cargo test -p docs_rs_web --lib handlers::listener::tests
```

On Linux, an additional opt-in test uses `systemd-socket-activate` to pass a real
listening socket to a child process. It checks that the inherited address takes
precedence over the fallback address and that a connection queued before the
child starts can be accepted and answered. This requires the systemd utility,
but does not require root or a running systemd manager:

```bash
SQLX_OFFLINE=true cargo test -p docs_rs_web --lib handlers::listener::tests::systemd_socket_activation -- --ignored
```

These tests require permission to bind local TCP sockets. The activation test
checks the socket-passing protocol, rather than deployment unit dependencies or
the graceful shutdown of the complete daemon.

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
