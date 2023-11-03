#!/usr/bin/env bash

set -e

# Just in case it's running, we stop the web server.
docker-compose stop web

docker-compose up -d db s3

# We add the information we need.
cargo run -- database migrate
cargo run -- build crate sysinfo 0.23.4
cargo run -- build crate sysinfo 0.23.5
cargo run -- build add-essential-files

# In case we don't have a `.env`, we create one.
if [ ! -f .env ]; then
cp .env.sample .env
fi

. .env
cargo run -- start-web-server &
SERVER_PID=$!

docker build . -f dockerfiles/Dockerfile-gui-tests -t gui_tests

# status="docker run . -v `pwd`:/build/out:ro gui_tests"
docker-compose run gui_tests
status=$?
exit $status
