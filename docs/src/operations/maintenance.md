# Common Maintenance Procedures

Most procedures use the [`docs_rs_admin` CLI](../binaries/admin_cli.md), which
connects directly to the database and S3. Run commands in an environment with
the production configuration and credentials.

## Build Queue

### Rebuild a Specific Crate

Add the crate release to the queue:

```console
$ docs_rs_admin queue add <CRATE_NAME> <VERSION>
```

The default priority is `5`, lower than the priority `0` used for newly
published releases. Override it when necessary:

```console
$ docs_rs_admin queue add <CRATE_NAME> <VERSION> --priority <PRIORITY>
```

**These days, crate authors can request rebuilds from the crates.io interface,
so queuing rebuilds like this is more the exception.**

### Manage Default Build Priorities

Default priorities apply to crate names matching a PostgreSQL pattern. Lower
numbers run first:

```console
$ docs_rs_admin queue default-priority list
$ docs_rs_admin queue default-priority get <CRATE_NAME>
$ docs_rs_admin queue default-priority set 'tokio-%' -5
$ docs_rs_admin queue default-priority remove 'tokio-%'
```

Repository-specific overrides match `repositories.name`:

```console
$ docs_rs_admin queue repository-priority list
$ docs_rs_admin queue repository-priority get <OWNER/REPOSITORY>
$ docs_rs_admin queue repository-priority set <OWNER/REPOSITORY> -10
$ docs_rs_admin queue repository-priority remove <OWNER/REPOSITORY>
```

## Pin a Nightly Toolchain

If the latest nightly breaks documentation builds, pin a known-good nightly:

```console
$ docs_rs_admin build set-toolchain nightly-YYYY-MM-DD
```

The builders read this setting from the database. No service restart is
required. To resume using the latest nightly, set the toolchain back to
`nightly`:

```console
$ docs_rs_admin build set-toolchain nightly
```

After resolving an incident, use `docs_rs_admin queue rebuild-broken-nightly` to
queue rebuilds for releases affected by one or more broken nightlies. See its
`--help` output for the required date range.

## Crate Administration

### Override Build Limits for a Crate

First, inspect the crate's current sandbox limit overrides:

```console
$ docs_rs_admin database limits get <CRATE_NAME>
```

Set the overrides with `docs_rs_admin database limits set`. This command
replaces the crate's complete set of overrides: options you omit are cleared.
Pass every existing override you want to retain. Memory is measured in bytes and
timeout in seconds. For example, to allow 8 GiB of memory and a 15-minute
timeout:

```console
$ docs_rs_admin database limits set <CRATE_NAME> \
    --memory 8589934592 --timeout 900
```

Use the `get`, `list`, and `remove` subcommands to inspect or remove overrides.

### Update Repository Statistics

Update GitHub and GitLab repository metadata with:

```console
$ docs_rs_admin database update-repository-fields
```

Set `DOCSRS_GITHUB_ACCESSTOKEN` to a GitHub access token before running this
command. `DOCSRS_GITLAB_ACCESSTOKEN` is optional; setting it raises the GitLab
API rate limit, while leaving it unset uses unauthenticated requests.

### Run or Revert Database Migrations

Apply all pending migrations with:

```console
$ docs_rs_admin database migrate
```

Pass a migration version to move the database to that precise version, including
reverting newer migrations:

```console
$ docs_rs_admin database migrate <MIGRATION_VERSION>
```

### Blacklist a Crate

Prevent future releases of a crate from being built:

```console
$ docs_rs_admin database blacklist add <CRATE_NAME>
```

Use the `list` and `remove` subcommands to inspect or change the blacklist.

> **Warning:** Blacklisting a crate does not remove content already published on
> the website.

When existing content must also be removed, use the
[`docs_rs_watcher`](../binaries/index-watcher.md) binary after blacklisting the
crate:

```console
$ docs_rs_watcher database delete crate <CRATE_NAME>
```

This deletes the crate's existing releases from the database and storage.
