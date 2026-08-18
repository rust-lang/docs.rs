# Admin CLI

The Admin CLI provides some helper commands to manage your docs.rs deployment.
It directly connects to the database & S3 and helps with some admin activities.

You can get help via:

```bash
$ docs_rs_admin --help # list of subcommands

Usage: docs_rs_admin <COMMAND>

Commands:
  build
  database  Database operations
  queue     Interactions with the build queue
  cdn
  help      Print this message or the help of the given subcommand(s)

$ docs_rs_admin queue --help # help for a subcommand
```
