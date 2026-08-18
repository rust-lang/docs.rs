# Admin CLI

The Admin CLI provides some helper commands to manage your docs.rs deployment.
It directly connects to the database & S3 and helps with some admin activities,
so _the CLI theoretically even when running on a separate machine without direct
access to the other services_.

## features

You can get help via:

```console
{{#include ../generated/docs_rs_admin--help.txt}}
```

## `build` subcommand

```console
{{#include ../generated/docs_rs_admin-build-help.txt}}
```

## `cdn` subcommand

```console
{{#include ../generated/docs_rs_admin-cdn-help.txt}}
```

## `database` subcommand

```console
{{#include ../generated/docs_rs_admin-database-help.txt}}
```

## `queue` subcommand

```console
{{#include ../generated/docs_rs_admin-queue-help.txt}}
```
