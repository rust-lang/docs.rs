FROM rust:slim

### STEP 1: Install dependencies ###
# Install packaged dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
  build-essential git curl cmake gcc g++ pkg-config libmagic-dev \
  libssl-dev zlib1g-dev sudo docker.io

### STEP 2: Setup build environment as new user ###
ENV CRATESFYI_PREFIX=/opt/docsrs/prefix
RUN mkdir -p $CRATESFYI_PREFIX

RUN mkdir -vp "$CRATESFYI_PREFIX"/documentations "$CRATESFYI_PREFIX"/public_html "$CRATESFYI_PREFIX"/sources
RUN git clone https://github.com/rust-lang/crates.io-index.git "$CRATESFYI_PREFIX"/crates.io-index
RUN git --git-dir="$CRATESFYI_PREFIX"/crates.io-index/.git branch crates-index-diff_last-seen

### STEP 3: Build the project ###
# Build the dependencies in a separate step to avoid rebuilding all of them
# every time the source code changes. This takes advantage of Docker's layer
# caching, and it works by copying the Cargo.{toml,lock} with dummy source code
# and doing a full build with it.
RUN mkdir -p /build/docs.rs /build/src/web/badge
WORKDIR /build
COPY Cargo.lock Cargo.toml ./
COPY src/web/badge src/web/badge/
RUN echo "fn main() {}" > src/main.rs && \
    echo "fn main() {}" > build.rs

RUN cargo fetch
RUN cargo build --release

### STEP 4: Build the website ###
# Dependencies are now cached, copy the actual source code and do another full
# build. The touch on all the .rs files is needed, otherwise cargo assumes the
# source code didn't change thanks to mtime weirdness.
RUN rm -rf src build.rs

COPY build.rs build.rs
RUN touch build.rs
COPY src src/
RUN find src -name "*.rs" -exec touch {} \;
COPY templates/style.scss templates/

RUN cargo build --release

ADD templates templates/
ADD css $CRATESFYI_PREFIX/public_html

ENV DOCS_RS_DOCKER=true
COPY docker-entrypoint.sh ./
ENTRYPOINT ["./docker-entrypoint.sh"]
CMD ["daemon", "--foreground"]
