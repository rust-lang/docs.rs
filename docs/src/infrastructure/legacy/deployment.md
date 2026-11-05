# Deployment

- SSH into the docs.rs server through the bastion host.
- Run `docs_rs_admin build lock` to pause new builds.
- Check [the build queue](https://docs.rs/releases/queue) until no builds are in
  progress.
- Run the `deploy-docs.rs` Bash script.
- If you update content that is typically cached, run
  `docs_rs_admin cdn purge all`.
- Run `docs_rs_admin build unlock` to resume builds.

Registry polling is controlled separately with `docs_rs_admin queue lock` and
`docs_rs_admin queue unlock`. Pausing registry polling does not pause builders,
and pausing builds does not stop registry polling.

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

## Configuration

To update a service's configuration, update the corresponding dotenv file. See
the [systemd setup](systemd.md) for details.
