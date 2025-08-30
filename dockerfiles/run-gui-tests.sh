#!/usr/bin/env bash

set -e

# Just in case it's running, we stop the web server.
docker compose stop web

docker compose up -d db s3

# If we have a .env file, we need to temporarily move it so
# it doesn't make sqlx fail compilation.
if [ -f .env ]; then
  mv .env .tmp.env
fi

# We add the information we need.
cargo run -- database migrate
cargo run -- build update-toolchain
cargo run -- build crate sysinfo 0.23.4
cargo run -- build crate sysinfo 0.23.5
cargo run -- build crate libtest 0.0.1
cargo run -- build add-essential-files

if [ -f .tmp.env ]; then
  mv .tmp.env .env
fi

# In case we don't have a `.env`, we create one.
if [ ! -f .env ]; then
  cp .env.sample .env
fi

. .env

set +e # We disable the "exit right away if command failed" setting.
cargo run -- start-web-server &
SERVER_PID=$!

# status="docker run . -v `pwd`:/build/out:ro gui_tests"
docker compose run --rm gui_tests
status=$?
kill $SERVER_PID
exit $status
