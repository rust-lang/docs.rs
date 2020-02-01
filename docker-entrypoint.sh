#!/usr/bin/env bash

set -euv

export CRATESFYI_PREFIX=/opt/docsrs/prefix
export DOCS_RS_DOCKER=true
export RUST_LOG=cratesfyi,rustwide=info
export PATH="$PATH:/build/target/release"

# Try migrating the database multiple times if it fails
# This avoids the docker container crashing the first time it's started with
# docker-compose, as PostgreSQL needs some time to initialize.
set +e
failed=0
while true; do
    if ! cratesfyi database migrate; then
        ((failed=failed + 1))
        if [ "${failed}" -eq 5 ]; then
            exit 1
        fi
        echo "failed to migrate the database"
        echo "waiting 1 second..."
        sleep 1
    else
        break
    fi
done
set -e

cratesfyi database update-search-index
cratesfyi database update-release-activity

if ! [ -d "${CRATESFYI_PREFIX}/crates.io-index/.git" ]; then
    git clone https://github.com/rust-lang/crates.io-index "${CRATESFYI_PREFIX}/crates.io-index"
    # Prevent new crates built before the container creation to be built
    git --git-dir="$CRATESFYI_PREFIX/crates.io-index/.git" branch crates-index-diff_last-seen
fi

cratesfyi build update-toolchain --only-first-time

cratesfyi "$@"
