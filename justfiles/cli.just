# a collection of just commands to wrap various docs.rs CLI commands, 
# and run them in a one-off docker container.
# _Which_ container depends on the command itself and its dependencies.
# Most service containers have their corresponding CLI container:
# * web -> cli 
# * builder-x -> builder-cli
# * registry-watcher -> registry-watcher-cli

# low-level helper to run any CLI command in its own one-off docker container, 
# ensuring that `db` and `s3` are running.
_cli service_name *args: _touch-docker-env _ensure_db_and_s3_are_running
  # dependencies in the docker-compose file are ignored 
  # when running a one-off service with `docker compose run`.
  # Instead we explicitly start any dependent services first via 
  # `_ensure_db_and_s3_are_running`.

  docker compose run --build --rm {{ service_name }} {{ args }}

# run any CLI command in its own one-off `cli` docker container. Args are passed to the container.
# Only for commands that just need `db` and `s3` and minimal system dependencies.
[group('cli')]
cli +args: _touch-docker-env cli-db-migrate
  just _cli cli {{ args }}

# Initialize the `docs.rs` database
[group('cli')]
[group('database')]
cli-db-migrate: 
  # intentially not using `cli` recipe, because it has a dependency on `cli-db-migrate`. 
  # Otherwise we would have a stack overflow / infinite recursion.
  # 
  # TODO: potential optimization: only run the container when we have to 
  # run migrations?
  just _cli cli database migrate

# add a release to the build queue
[group('cli')]
[group('queue')]
cli-queue-add crate_name crate_version:
  # only does things with the database, so can use the lightweight `cli` container.
  just cli queue add {{ crate_name }} {{ crate_version }}

# run builder CLI command in its own one-off `build-server` docker container.
# Uses a separate builder-cli container & workspace that doesn't conflict 
# with the continiously running build-servers.
[group('cli')]
[group('builder')]
cli-build +args: _touch-docker-env cli-db-migrate
  just _cli builder-cli {{ args }}

# set the nightly rust version to be used for builds. Format: `nightly-YYYY-MM-DD` 
# or just `nightly` for always using the latest nightly.  
[group('cli')]
[group('builder')]
cli-build-set-toolchain name only_first_time="false":
  #!/usr/bin/env bash
  set -euo pipefail

  FLAG=""
  if [ "{{only_first_time}}" = "true" ]; then FLAG="--only-first-time"; fi
  just cli-build build set-toolchain {{ name }} $FLAG

# update the toolchain in the builders
[group('cli')]
[group('builder')]
cli-build-update-toolchain:
  just cli-build build update-toolchain

# build & upload toolchain shared static resources
[group('cli')]
[group('builder')]
cli-build-add-essential-files:
  just cli-build build add-essential-files

# build a release
[group('cli')]
[group('builder')]
cli-build-crate name version:
  just cli-build build crate {{ name }} {{ version }}

# run registry-watcher CLI command in its own one-off `registry-watcher` docker container.
[group('cli')]
[group('watcher')]
cli-watcher +args: _touch-docker-env cli-db-migrate
  just _cli registry-watcher-cli {{ args }}

# Update last seen reference to the given hash, or the current `HEAD`. 
[group('cli')]
[group('queue')]
cli-queue-reset-last-seen-ref ref="--head": 
  just cli-watcher queue set-last-seen-reference {{ ref }}

# find differences between crates.io and our own database, and fix them on our side.
[group('cli')]
[group('database')]
cli-db-synchronize *args:
  just cli-watcher database synchronize {{ args }}
