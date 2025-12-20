# syntax=docker/dockerfile:1

FROM rust:1.92-slim-trixie AS chef

# We only pay the installation cost once, 
# it will be cached from the second build onwards
RUN cargo install cargo-chef 
WORKDIR /build

#################
#  Plan stage  #
#################

FROM chef AS planner
WORKDIR /build

COPY . .
RUN cargo chef prepare --recipe-path recipe.json

#################
#  Build stage  #
#################

FROM chef AS build
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

WORKDIR /build

COPY --from=planner /build/recipe.json recipe.json

# Configure linking to use mold instead for speed (need to use clang because gcc
# is too old on this image)
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=-Clink-arg=-fuse-ld=mold


ARG PROFILE=release

# Build dependencies - this is the caching Docker layer!
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=locked \
    --mount=type=cache,target=/build/target,id=cargo-target,sharing=locked \
    cargo chef cook --profile=$PROFILE --recipe-path /build/recipe.json

ENV PATH=/root/.cargo/bin:$PATH

# get the git SHA from the build args, for our generated version numbers
ARG GIT_SHA=dev
ENV GIT_SHA=$GIT_SHA

ENV SQLX_OFFLINE=true

# Build application
COPY . .
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

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]

COPY --from=build /artifacts/cratesfyi /usr/local/bin
