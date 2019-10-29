#!/bin/sh

set -euv

export CRATESFYI_DATABASE_URL=postgresql://cratesfyi:password@db
export CRATESFYI_CONTAINER_NAME=cratesfyi-container
export CRATESFYI_GITHUB_USERNAME=
export CRATESFYI_GITHUB_ACCESSTOKEN=
export RUST_LOG=cratesfyi,rustwide=info
export PATH="$PATH:/build/target/release"

cratesfyi database migrate
cratesfyi database update-search-index
cratesfyi database update-release-activity

cratesfyi "$@"
