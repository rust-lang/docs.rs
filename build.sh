#!/bin/sh
# Put this file into chroot_dir/home/onur/.build.sh and make it executable

set -e

cd $HOME/$2

case "$1" in
  build)
    cargo doc --verbose --no-deps
    ;;
  clean)
    cargo clean --verbose
    ;;
esac
