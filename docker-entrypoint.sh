#!/bin/sh
BIN=target/release/cratesfyi
$BIN database migrate
$BIN build crate rand 0.5.5
$BIN database update-search-index
$BIN database update-release-activity
exec $BIN "$@"
