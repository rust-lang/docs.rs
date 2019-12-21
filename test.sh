#!/bin/sh
# test script for docs.rs
# requires the following tools installed:
# - docker-compose
# - curl
# - pup (https://github.com/EricChiang/pup)

set -euv

HOST=http://localhost:3000

cp .env .env.bak

cleanup() {
	mv .env.bak .env
	docker-compose down
}

trap cleanup exit
echo DOCS_RS_FAST_INIT=true >> .env

docker-compose build
# run a dummy command so that the next background command starts up quickly
# and doesn't try to compete with `build` commands
docker-compose run web -- --help || true
# this never exits, run it in the background
docker-compose up -d
# ensure that if the web server doesn't start up we catch the error
for i in $(seq 1 5); do
	curl -I $HOST && break
	# if we've tried 5 times, exit with error
	! [ "$i" = 5 ]
	sleep 5
done

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
build rand 0.6.5
build pyo3 0.2.7
build pyo3 0.8.3

# small wrapper around curl to hide extraneous output
curl() {
    command curl -s -o /dev/null "$@"
}

# exit with a failure message
die() {
	echo "$@"
	exit 1
}

# give the HTTP status of a page hosted locally
status() {
	curl -I -w "%{http_code}" "$HOST$1"
}

# give the URL a page hosted locally redirects to
# if the page does not redirect, returns the same page it was given
redirect() {
	curl -IL -w "%{url_effective}" "$HOST$1"
}

version_redirect() {
    command curl -s "$HOST$1" | pup "form ul li a.warn attr{href}"
}

assert_eq() {
	[ "$1" = "$2" ] || die "expected '$1' to be '$2'"
}

assert_status() {
	assert_eq "$(status "$1")" "$2"
}
assert_redirect() {
	assert_eq "$(redirect "$1")" "$HOST$2"
}
assert_version_redirect() {
	assert_eq "$(version_redirect "$1")" "$2"
}

# make sure the crates built successfully
for crate in /rand/0.7.2/rand /rstest/0.4.1/rstest /sysinfo/0.10.0/sysinfo /constellation-rs/0.1.8/constellation; do
	echo "$crate"
	assert_status "$crate/" 200
done

assert_redirect /bat/0.12.1/bat /crate/bat/0.12.1

# make sure it shows source code for rustdoc
assert_status /constellation-rs/0.1.8/src/constellation/lib.rs.html 200
# make sure it shows source code for docs.rs
assert_status /crate/constellation-rs/0.1.8/source/ 200
# with or without trailing slashes
assert_redirect /crate/constellation-rs/0.1.8/source /crate/constellation-rs/0.1.8/source/

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
# check std library redirects
for crate in std alloc core proc_macro test; do
	echo $crate
	# with or without slash
	assert_eq "$(redirect /$crate)" https://doc.rust-lang.org/stable/$crate/
	assert_eq "$(redirect /$crate/)" https://doc.rust-lang.org/stable/$crate/
done
