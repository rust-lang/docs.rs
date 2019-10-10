FROM rust:slim

### STEP 1: Install dependencies ###
# Install packaged dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
  build-essential git curl cmake gcc g++ pkg-config libmagic-dev \
  libssl-dev zlib1g-dev

### STEP 2: Create user ###
ENV HOME=/home/cratesfyi
RUN adduser --home $HOME --disabled-login --disabled-password --gecos "" cratesfyi

### STEP 3: Setup build environment as new user ###
ENV CRATESFYI_PREFIX=/home/cratesfyi/prefix
ENV CRATESFYI_DATABASE_URL=postgresql://cratesfyi:password@db
ENV CRATESFYI_CONTAINER_NAME=cratesfyi-container
ENV CRATESFYI_GITHUB_USERNAME=
ENV CRATESFYI_GITHUB_ACCESSTOKEN=
ENV RUST_LOG=cratesfyi
ENV PATH="$PATH:$HOME/docs.rs/target/release"

RUN mkdir $CRATESFYI_PREFIX
RUN chown cratesfyi:cratesfyi "$CRATESFYI_PREFIX"

USER cratesfyi
RUN mkdir -vp "$CRATESFYI_PREFIX"/documentations "$CRATESFYI_PREFIX"/public_html "$CRATESFYI_PREFIX"/sources
RUN git clone https://github.com/rust-lang/crates.io-index.git "$CRATESFYI_PREFIX"/crates.io-index
RUN git --git-dir="$CRATESFYI_PREFIX"/crates.io-index/.git branch crates-index-diff_last-seen

### STEP 4: Build the project ###
# Build the dependencies in a separate step to avoid rebuilding all of them
# every time the source code changes. This takes advantage of Docker's layer
# caching, and it works by copying the Cargo.{toml,lock} with dummy source code
# and doing a full build with it.
RUN mkdir -p ~/docs.rs ~/docs.rs/src/web/badge
WORKDIR $HOME/docs.rs
COPY --chown=cratesfyi Cargo.lock Cargo.toml ./
COPY --chown=cratesfyi src/web/badge src/web/badge/
RUN echo "fn main() {}" > src/main.rs && \
    echo "fn main() {}" > build.rs

RUN cargo fetch
RUN cargo build --release

### STEP 5: Install docker on the container ###
# this is necessary for rustwide
USER root
RUN apt-get install -y --no-install-recommends apt-transport-https ca-certificates curl gnupg2 software-properties-common
RUN curl -fsSL https://download.docker.com/linux/debian/gpg | apt-key add -
RUN add-apt-repository \
       "deb [arch=amd64] https://download.docker.com/linux/debian \
       $(lsb_release -cs) stable"
RUN apt-get update && apt-get install -y --no-install-recommends docker-ce
RUN usermod -a -G docker cratesfyi

### STEP 6: Build the website ###
USER cratesfyi

# Dependencies are now cached, copy the actual source code and do another full
# build. The touch on all the .rs files is needed, otherwise cargo assumes the
# source code didn't change thanks to mtime weirdness.
RUN rm -rf src build.rs
COPY --chown=cratesfyi src src/
COPY --chown=cratesfyi build.rs build.rs
COPY --chown=cratesfyi templates templates/
RUN find . -name "*.rs" -exec touch {} \; && cargo build --release

USER root
RUN /etc/init.d/docker start

USER cratesfyi
ENV DOCS_RS_DOCKER=true
RUN RUST_LOG=debug cargo run --release -- build add-essential-files

RUN cargo run --release -- database migrate
RUN cargo run --release -- build crate rand 0.5.5
RUN cargo run --release -- database update-search-index
RUN cargo run --release -- database update-release-activity

### STEP 7: Start the webserver ###
USER root
COPY setup/cratesfyi.server /etc/systemd/system/cratesfyi.service
RUN systemctl start cratesfyi.server
