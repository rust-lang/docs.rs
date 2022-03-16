#!/usr/bin/env bash

# Just in case it's running, we stop the web server.
docker-compose stop web

docker-compose up -d db s3

# We add the information we need.
cargo run -- database migrate
docker-compose run web build crate sysinfo 0.23.4
docker-compose run web build crate sysinfo 0.23.5
docker-compose run web build add-essential-files
docker-compose build web

# In case we don't have a `.env`, we create one.
if [ ! -f .env ]
then
cp .env.sample .env
source .env
fi

docker-compose up -d web

cargo run -- start-web-server &
SERVER_PID=$!

docker build . -f dockerfiles/Dockerfile-gui-tests -t gui_tests

echo "Sleeping a bit to be sure the web server will be started..."
sleep 5

# status="docker run . -v `pwd`:/build/out:ro gui_tests"
docker-compose run gui_tests
status=$?
kill -9 $SERVER_PID
exit $status
