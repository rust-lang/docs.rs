# Deployment

- SSH into the docs.rs server through the bastion host.
- Run `docs_rs_admin build lock` to lock the build queue.
- Check [the build queue](https://docs.rs/releases/queue) until no builds are in
  progress.
- Run the `deploy-docs.rs` Bash script.
- If you update content that is typically cached, run
  `docs_rs_admin cdn purge all`.
- Run `docs_rs_admin build unlock` to resume builds.

`deploy-docs.rs` will:

- tag the old `HEAD` as the previous release with `git tag`,
- pull the repository with `git pull`,
- run a build,
- copy the binaries,
- restart the systemd services,
- run database migrations, and
- purge the CDN of static content stored in the repository.

There is also a `revert-docs.rs` script that reverts to the previously tagged
release. _It doesn't revert database migrations._

## systemd service setup

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

```bash
docker container prune --force
docker image prune --force
cargo-sweep sweep /home/ubuntu/docs.rs --installed
```
