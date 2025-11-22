# syntax=docker/dockerfile:1

# To produce a smaller image this Dockerfile contains two separate stages: in
# the first one all the build dependencies are installed and docs.rs is built,
# while in the second one just the runtime dependencies are installed, with the
# binary built in the previous stage copied there.
#
# As of 2019-10-29 this reduces the image from 2.8GB to 500 MB.


#################
#  Build stage  #
#################

FROM rust:1.91-slim-trixie AS build

ENV DEBIAN_FRONTEND=noninteractive

# Install packaged dependencies
# hadolint ignore=DL3008
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    build-essential \
    git \
    curl \
    cmake \
    gcc \
    g++ \
    pkg-config \
    libmagic-dev \
    libssl-dev \
    zlib1g-dev \
    ca-certificates \
    mold \
    clang

ENV PATH=/root/.cargo/bin:$PATH

# get the git SHA from the build args, for our generated version numbers
ARG GIT_SHA=dev
ENV GIT_SHA=$GIT_SHA

# Configure linking to use mold instead for speed (need to use clang because gcc
# is too old on this image)
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=-Clink-arg=-fuse-ld=mold

# Build the dependencies in a separate step to avoid rebuilding all of them
# every time the source code changes. This takes advantage of Docker's layer
# caching, and it works by copying the Cargo.{toml,lock} with dummy source code
# and doing a full build with it.
WORKDIR /build
COPY benches benches
COPY Cargo.lock Cargo.toml ./
COPY crates crates
RUN mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/cratesfyi.rs && \
    echo "fn main() {}" > build.rs

ARG PROFILE=release
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/build/target,id=cargo-target,sharing=locked \
    cargo build --profile=$PROFILE

# Dependencies are now cached, copy the actual source code and do another full
# build. The touch on all the .rs files is needed, otherwise cargo assumes the
# source code didn't change thanks to mtime weirdness.
RUN rm -rf src build.rs

COPY build.rs build.rs
RUN touch build.rs
COPY src src/
RUN find src -name "*.rs" -exec touch {} \;
COPY templates templates/
COPY vendor vendor/
COPY static static/
COPY assets assets/
COPY .sqlx .sqlx/
COPY migrations migrations/

ARG PROFILE_DIR=release
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/build/target,id=cargo-target,sharing=locked \
    mkdir /artifacts && \
    cargo build --profile=$PROFILE && \
    cp /build/target/$PROFILE_DIR/cratesfyi /artifacts/

######################
#  Web server stage  #
######################

FROM debian:trixie-slim AS web-server

ENV DEBIAN_FRONTEND=noninteractive

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y \
        --no-install-recommends \
        ca-certificates \
        curl \
        tini \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /srv/docsrs

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]
CMD ["start-web-server", "0.0.0.0:3000"]

COPY --from=build /artifacts/cratesfyi /usr/local/bin
COPY static /srv/docsrs/static
COPY vendor /srv/docsrs/vendor

########################
#  Build server stage  #
########################
# * includes docker-cli, but expects the docker engine to be mapped into 
#   the container from the outside, e.g. via 
#
#     volumes:
#       - "/var/run/docker.sock:/var/run/docker.sock"

FROM debian:trixie-slim AS build-server

ENV DEBIAN_FRONTEND=noninteractive

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y \
        --no-install-recommends \
        ca-certificates \
        tini \
        curl \
        docker-cli \
        build-essential \
        gcc \
        pkg-config \
        libssl-dev \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]
CMD ["start-build-server", "0.0.0.0:3000"]

COPY --from=build /artifacts/cratesfyi /usr/local/bin


############################
#  Registry watcher stage  #
############################

FROM debian:trixie-slim AS registry-watcher

ENV DEBIAN_FRONTEND=noninteractive

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y \
        --no-install-recommends \
        ca-certificates \
        tini \
        curl \
        git \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]
CMD ["start-registry-watcher", "0.0.0.0:3000", "--repository-stats-updater=enabled", "--cdn-invalidator=enabled", "--queue-rebuilds=enabled"]

COPY --from=build /artifacts/cratesfyi /usr/local/bin

###############
#  CLI stage  #
###############
# This stage is used to run one-off commands like database migrations.
# not suited for commands that need: 
# * the crates.io index, or
# * need to run builds.
# for these, use the build-server stage, or registry-watcher stage instead.

FROM debian:trixie-slim AS cli

ENV DEBIAN_FRONTEND=noninteractive

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y \
        --no-install-recommends \
        ca-certificates \
        tini \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /srv/docsrs
# copy migrations so we can run them via CLI script
COPY migrations migrations/

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]

COPY --from=build /artifacts/cratesfyi /usr/local/bin
