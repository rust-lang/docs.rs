#!/usr/bin/env bash
# test script for docs.rs
# requires the following tools installed:
# - docker-compose
# - curl
# - pup (https://github.com/EricChiang/pup)

set -euv

docker-compose build
# this never exits, run it in the background
# TODO: catch errors if `up` failed
docker-compose up --build -d

# build a crate and store it in the database if it does not already exist
build() {
	docker-compose run web build --skip --skip-if-log-exists crate "$@"
}

# build a few types of crates
# library
build rand 0.7.2
# binary
build bat 0.12.1
# proc-macro
build rstest 0.4.1
# multiple crate types
build sysinfo 0.10.0
# renamed crate
build constellation-rs 0.1.8
# used later for latest version
build rand 0.7.1
build pyo3 0.2.7
build pyo3 0.8.3

HOST=localhost:3000

# small wrapper around curl to hide extraneous output
curl() {
    command curl -s -o /dev/null "$@"
}

# give the HTTP status of a page hosted locally
status() {
	curl -I -w %{http_code} "$HOST/$1"
}

# give the URL a page hosted locally redirects to
# if the page does not redirect, returns the same page it was given
redirect() {
	curl -IL -w %{url_effective} "$HOST/$1"
}

version_redirect() {
    curl "$1" | pup "form ul li a.warn attr{href}"
}

assert_status() {
	[ "$(status "$1")" = "$2" ]
}
assert_redirect() {
	[ "$(redirect "$1")" = "$HOST/$2" ]
}
assert_version_redirect() {
	[ "$(version_redirect "$1")" = "$HOST/$2" ]
}

# make sure the crates built successfully
for crate in /rand/0.7.2/rand /rstest/0.12.1/rstest /sysinfo/0.10.0/sysinfo /constellation-rs/0.1.8/constellation; do
	assert_status "$crate" 200
done

assert_redirect /bat/0.12.1/bat /crate/bat/0.12.1

# make sure it shows source code for rustdoc
assert_status /constellation-rs/0.1.8/src/constellation/lib.rs.html 200
# make sure it shows source code for docs.rs
assert_status /crate/constellation-rs/0.1.8/source/ 200
# with or without trailing slashes
assert_status /crate/constellation-rs/0.1.8/source 200

# check 'Go to latest version' keeps the current page
assert_version_redirect /rand/0.6.5/rand/trait.Rng.html \
                        /rand/0.7.2/rand/trait.Rng.html
# and the current platform
assert_version_redirect /rand/0.6.5/x86_64-unknown-linux-gnu/rand/fn.thread_rng.html \
                        /rand/0.7.2/x86_64-unknown-linux-gnu/rand/fn.thread_rng.html
# latest version searches for deleted items
assert_version_redirect /rand/0.6.5/rand/rngs/struct.JitterRng.html \
                        "/rand/0.7.2/rand/?search=JitterRng"
# for renamed items
assert_version_redirect /pyo3/0.2.7/pyo3/exc/struct.ArithmeticError.html \
                        "/pyo3/0.8.3/pyo3/?search=ArithmeticError"

docker-compose down
