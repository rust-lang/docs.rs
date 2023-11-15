# To produce a smaller image this Dockerfile contains two separate stages: in
# the first one all the build dependencies are installed and docs.rs is built,
# while in the second one just the runtime dependencies are installed, with the
# binary built in the previous stage copied there.
#
# As of 2019-10-29 this reduces the image from 2.8GB to 500 MB.

#################
#  Build stage  #
#################

FROM ubuntu:22.04 AS build

# Install packaged dependencies
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential git curl cmake gcc g++ pkg-config libmagic-dev \
    libssl-dev zlib1g-dev ca-certificates mold clang

# Install the stable toolchain with rustup
RUN curl https://sh.rustup.rs >/tmp/rustup-init && \
    chmod +x /tmp/rustup-init && \
    /tmp/rustup-init -y --no-modify-path --default-toolchain stable --profile minimal
ENV PATH=/root/.cargo/bin:$PATH

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
COPY crates/metadata crates/metadata/
COPY crates/font-awesome-as-a-crate crates/font-awesome-as-a-crate
RUN mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/cratesfyi.rs && \
    echo "fn main() {}" > build.rs

RUN cargo fetch

ARG PROFILE=release
RUN cargo build --profile=$PROFILE

# Dependencies are now cached, copy the actual source code and do another full
# build. The touch on all the .rs files is needed, otherwise cargo assumes the
# source code didn't change thanks to mtime weirdness.
RUN rm -rf src build.rs

COPY .git .git
COPY build.rs build.rs
RUN touch build.rs
COPY src src/
RUN find src -name "*.rs" -exec touch {} \;
COPY templates templates/
COPY vendor vendor/
COPY assets assets/
COPY .sqlx .sqlx/
COPY migrations migrations/

RUN cargo build --profile=$PROFILE

######################
#  Web server stage  #
######################

FROM ubuntu:22.04 AS web-server

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
        ca-certificates \
        curl \
        tini \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /srv/docsrs

# Tini is a small init binary to properly handle signals
ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]
CMD ["start-web-server", "0.0.0.0:80"]

ARG PROFILE_DIR=release
COPY --from=build /build/target/$PROFILE_DIR/cratesfyi /usr/local/bin
COPY static /srv/docsrs/static
COPY templates /srv/docsrs/templates
COPY vendor /srv/docsrs/vendor

########################
#  Build server stage  #
########################

FROM ubuntu:22.04 AS build-server

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
        ca-certificates \
        tini \
        curl \
        docker.io \
        build-essential \
        gcc \
        pkg-config \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Tini is a small init binary to properly handle signals
ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]
CMD ["start-build-server"]

ARG PROFILE_DIR=release
COPY --from=build /build/target/$PROFILE_DIR/cratesfyi /usr/local/bin

############################
#  Registry watcher stage  #
############################

FROM ubuntu:22.04 AS registry-watcher

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
        ca-certificates \
        tini \
        curl \
        git \
    && rm -rf /var/lib/apt/lists/*

# Tini is a small init binary to properly handle signals
ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]
CMD ["start-registry-watcher", "--repository-stats-updater=enabled", "--cdn-invalidator=enabled"]

ARG PROFILE_DIR=release
COPY --from=build /build/target/$PROFILE_DIR/cratesfyi /usr/local/bin

###############
#  CLI stage  #
###############

FROM ubuntu:22.04 AS cli

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
        ca-certificates \
        tini \
    && rm -rf /var/lib/apt/lists/*

ENTRYPOINT ["/usr/bin/tini", "/usr/local/bin/cratesfyi", "--"]

ARG PROFILE_DIR=release
COPY --from=build /build/target/$PROFILE_DIR/cratesfyi /usr/local/bin
