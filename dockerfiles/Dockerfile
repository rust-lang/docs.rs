# syntax=docker/dockerfile:1

FROM rust:1.93-slim-trixie AS chef

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
    cargo build --profile=$PROFILE --workspace && \
    cp /build/target/$PROFILE_DIR/cratesfyi /artifacts/ && \
    cp /build/target/$PROFILE_DIR/docs_rs_* /artifacts/

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

# copy the new binary, will also be used as default entrypoint.
COPY --from=build /artifacts/docs_rs_web /usr/local/bin/

# also copy the old legacy binary, so we can fall back to using it 
# by just overwriting the entrypoint & CMD when running this container.
COPY --from=build /artifacts/cratesfyi /usr/local/bin/

# copy necessary assets
COPY crates/bin/docs_rs_web/static /srv/docsrs/static
COPY crates/bin/docs_rs_web/vendor /srv/docsrs/vendor

ENTRYPOINT ["/usr/bin/tini"]
CMD ["/usr/local/bin/docs_rs_web", "--", "0.0.0.0:3000"]


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

# copy the new binary, will also be used as default entrypoint.
COPY --from=build /artifacts/docs_rs_builder /usr/local/bin/

# also copy the old legacy binary, so we can fall back to using it 
# by just overwriting the entrypoint & CMD when running this container.
COPY --from=build /artifacts/cratesfyi /usr/local/bin/

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/docs_rs_builder", "--"]
CMD ["start"]


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

# copy the new binary, will also be used as default entrypoint.
COPY --from=build /artifacts/docs_rs_watcher /usr/local/bin/

# also copy the old legacy binary, so we can fall back to using it 
# by just overwriting the entrypoint & CMD when running this container.
COPY --from=build /artifacts/cratesfyi /usr/local/bin/

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/docs_rs_watcher", "--"]
CMD ["start", "--repository-stats-updater", "--queue-rebuilds"]

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

COPY crates/bin/docs_rs_admin/bin/docs_rs_release.sh /usr/local/bin/

COPY --from=build /artifacts/docs_rs_admin /usr/local/bin/
COPY --from=build /artifacts/cratesfyi /usr/local/bin

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/docs_rs_admin", "--"]
