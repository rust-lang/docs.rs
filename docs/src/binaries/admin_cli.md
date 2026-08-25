# Admin CLI

The Admin CLI provides helper commands for managing a docs.rs deployment. It
connects directly to the database and S3, so _it can theoretically run on a
separate machine without direct access to the other running web or
build-servers_.

For task-oriented instructions that use these commands, see the
[common maintenance procedures](../operations/maintenance.md).

## Features

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
