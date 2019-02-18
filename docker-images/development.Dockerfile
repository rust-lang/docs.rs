FROM ubuntu:bionic

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get -q update && apt-get -y dist-upgrade
RUN apt-get install -yq --no-install-recommends \
        cmake curl cmake gcc g++ git libmagic-dev \
        libssl-dev pkg-config ca-certificates

# Install rust
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain nightly-2019-02-05

ENV PATH=/root/.cargo/bin:$PATH
ENV CARGO_TARGET_DIR=/root/docsrs-target

# Install rustfmt and clippy
RUN rustup component add rustfmt
RUN rustup component add clippy
RUN rm -rf /root/.cargo/git /root/.cargo/registry

WORKDIR /docsrs
