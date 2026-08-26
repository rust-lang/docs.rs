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
macOS, Windows, or an incompatible architecture, run the builder through Docker
Compose instead:

```console
$ just compose-up-builder
$ just cli-build-update-toolchain
$ just cli-build-crate regex 1.3.1
```

## Tests fail or time out unexpectedly

The test suite needs at least 4096 open file descriptors. Check the current
limit and raise it for the current shell when necessary:

```console
$ ulimit -n
$ ulimit -n 4096
```
