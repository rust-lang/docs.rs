#!/bin/sh

set -euv

export CRATESFYI_PREFIX=/opt/docsrs/prefix
export DOCS_RS_DOCKER=true
export RUST_LOG=cratesfyi,rustwide=info
export PATH="$PATH:/build/target/release"

cratesfyi database migrate
cratesfyi database update-search-index
cratesfyi database update-release-activity

if ! [ -d "${CRATESFYI_PREFIX}/crates.io-index/.git" ]; then
    git clone https://github.com/rust-lang/crates.io-index "${CRATESFYI_PREFIX}/crates.io-index"
fi

cratesfyi "$@"
