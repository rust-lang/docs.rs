# syntax=docker/dockerfile:1

FROM rust:1.98-slim-trixie AS chef

# We only pay the installation cost once,
# it will be cached from the second build onwards
RUN cargo install --locked --version 0.1.78 cargo-chef
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
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        git \
        cmake \
        pkg-config \
        libmagic-dev \
        libssl-dev \
        zlib1g-dev \
        ca-certificates \
        mold \
        clang \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY --from=planner /build/recipe.json recipe.json

# Configure linking to use mold instead for speed (need to use clang because gcc
# is too old on this image)
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=-Clink-arg=-fuse-ld=mold


ARG PROFILE=release
ARG PROFILE_DIR=release

ENV SQLX_OFFLINE=true

##########################
#  Cached dependencies   #
##########################

FROM build AS dependencies

# Do not use a cache mount here: keep cargo-chef's /build/target output in an
# ordinary image layer so remote cache exporters retain it. Each binary build
# below uses this directory as the initial contents of its mutable target cache.
RUN cargo chef cook --profile=$PROFILE --recipe-path /build/recipe.json

##########################
#  Application binaries  #
##########################

FROM dependencies AS application-build

# Get the git SHA from the build args for our generated version numbers. Keep
# it after the dependency layer so each commit only invalidates application builds.
ARG GIT_SHA=dev
ENV GIT_SHA=$GIT_SHA

FROM application-build AS build-web-server
COPY . .
# Seed the target cache from cargo-chef's dependency layer. BuildKit keeps later
# changes to this cache for fast incremental rebuilds on a persistent local
# builder. Ephemeral CI builders generally do not retain the cache mount between
# runs, but still receive the cargo-chef dependency baseline from the image layer.
RUN --mount=type=cache,target=/build/target,id=docs-rs-cargo-target,from=dependencies,source=/build/target,sharing=locked \
    mkdir /artifacts && \
    cargo build --profile=$PROFILE --package docs_rs_web && \
    cp /build/target/$PROFILE_DIR/docs_rs_web /artifacts/

FROM application-build AS build-build-server
COPY . .
RUN --mount=type=cache,target=/build/target,id=docs-rs-cargo-target,from=dependencies,source=/build/target,sharing=locked \
    mkdir /artifacts && \
    cargo build --profile=$PROFILE --package docs_rs_builder && \
    cp /build/target/$PROFILE_DIR/docs_rs_builder /artifacts/

FROM application-build AS build-registry-watcher
COPY . .
RUN --mount=type=cache,target=/build/target,id=docs-rs-cargo-target,from=dependencies,source=/build/target,sharing=locked \
    mkdir /artifacts && \
    cargo build --profile=$PROFILE --package docs_rs_watcher && \
    cp /build/target/$PROFILE_DIR/docs_rs_watcher /artifacts/

FROM application-build AS build-cli
COPY . .
RUN --mount=type=cache,target=/build/target,id=docs-rs-cargo-target,from=dependencies,source=/build/target,sharing=locked \
    mkdir /artifacts && \
    cargo build --profile=$PROFILE --package docs_rs_admin && \
    cp /build/target/$PROFILE_DIR/docs_rs_admin /artifacts/

########################
#  Runtime base stage  #
########################

FROM debian:trixie-slim AS runtime-base

ENV DEBIAN_FRONTEND=noninteractive

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y \
        --no-install-recommends \
        ca-certificates \
        libssl3t64 \
        tini \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

######################
#  Web server stage  #
######################

FROM runtime-base AS web-server

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y \
        --no-install-recommends \
        curl \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /srv/docsrs

# copy the new binary, will also be used as default entrypoint.
COPY --from=build-web-server /artifacts/docs_rs_web /usr/local/bin/

# copy necessary assets
COPY crates/bin/docs_rs_web/static /srv/docsrs/static
COPY crates/bin/docs_rs_web/vendor /srv/docsrs/vendor

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/docs_rs_web"]
CMD ["0.0.0.0:3000"]


########################
#  Build server stage  #
########################
# * includes docker-cli, but expects the docker engine to be mapped into
#   the container from the outside, e.g. via
#
#     volumes:
#       - "/var/run/docker.sock:/var/run/docker.sock"

FROM runtime-base AS build-server

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y \
        --no-install-recommends \
        curl \
        docker-cli \
        build-essential \
        gcc \
        pkg-config \
        libssl-dev \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# copy the new binary, will also be used as default entrypoint.
COPY --from=build-build-server /artifacts/docs_rs_builder /usr/local/bin/

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/docs_rs_builder"]
CMD ["start"]


############################
#  Registry watcher stage  #
############################

FROM runtime-base AS registry-watcher

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y \
        --no-install-recommends \
        git \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# copy the new binary, will also be used as default entrypoint.
COPY --from=build-registry-watcher /artifacts/docs_rs_watcher /usr/local/bin/

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/docs_rs_watcher"]
CMD ["start", "--repository-stats-updater", "--queue-rebuilds"]

###############
#  CLI stage  #
###############
# This stage is used to run one-off commands like database migrations.
# not suited for commands that need:
# * the crates.io index, or
# * need to run builds.
# for these, use the build-server stage, or registry-watcher stage instead.

FROM runtime-base AS cli

WORKDIR /srv/docsrs

COPY crates/bin/docs_rs_admin/bin/docs_rs_release.sh /usr/local/bin/

COPY --from=build-cli /artifacts/docs_rs_admin /usr/local/bin/

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/docs_rs_admin"]
