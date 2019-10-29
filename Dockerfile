FROM ubuntu:bionic

### STEP 1: Install dependencies ###
# Install packaged dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
  build-essential git curl cmake gcc g++ pkg-config libmagic-dev \
  libssl-dev zlib1g-dev sudo ca-certificates docker.io

# Install the stable toolchain with rustup
RUN curl https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init >/tmp/rustup-init && \
    chmod +x /tmp/rustup-init && \
    /tmp/rustup-init -y --no-modify-path --default-toolchain stable
ENV PATH=/root/.cargo/bin:$PATH

### STEP 2: Setup build environment as new user ###
RUN mkdir -p /opt/docsrs/prefix

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
COPY css /opt/docsrs/prefix/public_html

COPY docker-entrypoint.sh ./
ENTRYPOINT ["./docker-entrypoint.sh"]
CMD ["daemon", "--foreground"]
