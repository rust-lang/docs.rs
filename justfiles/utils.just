
_ensure_db_and_s3_are_running: _touch-docker-env
    # dependencies in the docker-cli file are ignored
    # here. Instead we explicitly start any dependent services first.
    docker compose up -d db s3 --wait

_touch-docker-env:
  touch .docker.env

# helper recipe to ensure a CLI tool is installed.
# * Accepts multiple names
# * uses `cargo binstall` if it exists.
#
# example usage:
# ```
# _ensure_mdbook_installed: (_ensure_cargo_installed "mdbook" "mdbook-linkcheck2")
# ```
_ensure_cargo_installed *packages:
    #!/usr/bin/env bash
    set -euo pipefail

    for package in {{ packages }} ; do
      if command -v "$package" >/dev/null 2>&1; then
        continue
      fi

      if command -v cargo-binstall >/dev/null 2>&1; then
        cargo binstall -y "$package"
      else
        cargo install "$package"
      fi
    done

