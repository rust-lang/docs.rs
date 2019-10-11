#!/bin/sh

set -euv

USER=cratesfyi
BIN=target/release/cratesfyi

export CRATESFYI_PREFIX=/home/cratesfyi/prefix
export CRATESFYI_DATABASE_URL=postgresql://cratesfyi:password@db
export CRATESFYI_CONTAINER_NAME=cratesfyi-container
export CRATESFYI_GITHUB_USERNAME=
export CRATESFYI_GITHUB_ACCESSTOKEN=
export RUST_LOG=cratesfyi
export PATH="$PATH:$HOME/docs.rs/target/release"

sudo -E -u $USER $BIN database migrate
# rustwide needs to run as root
$BIN build crate rand 0.5.5

sudo -E -u $USER $BIN database update-search-index
sudo -E -u $USER $BIN database update-release-activity
exec $BIN daemon
