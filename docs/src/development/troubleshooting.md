# Troubleshooting Local Development

## Docker reports `exec user process caused "no such file or directory"`

Check whether the affected entrypoint script has CRLF line endings. A CRLF line
ending changes a hashbang such as `#!/bin/sh` into a request for the nonexistent
interpreter `/bin/sh\r` inside the container.

Configure Git to retain LF line endings for files checked out on Windows:

```console
$ git config core.autocrlf input
```

Then check out a fresh copy of the affected file.

## A builder command reports `Exec format error`

Running builds directly on the host requires a compatible Linux environment. On
platforms other than amd64 Linux, the `builder` recipe defaults to Docker mode
automatically. If local mode was selected explicitly or the detected host is
still incompatible, override it for the command:

```console
$ DOCSRS_BUILDER_CLI_MODE=docker just builder build update-toolchain
$ DOCSRS_BUILDER_CLI_MODE=docker just builder build crate regex 1.3.1
```

To use Docker mode for all builder commands in this checkout, add the following
to `.env`:

```dotenv
DOCSRS_BUILDER_CLI_MODE=docker
```

See [CLI execution modes](local-configuration.md#cli-execution-modes) for the
mode defaults and configuration behavior. To bypass mode selection entirely, run
the Compose service explicitly with `just docker-run builder-cli …`.

## Tests fail or time out unexpectedly

The test suite needs at least 4096 open file descriptors. Check the current
limit and raise it for the current shell when necessary:

```console
$ ulimit -n
$ ulimit -n 4096
```
