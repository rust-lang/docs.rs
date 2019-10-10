FROM rust:slim

### STEP 1: Install dependencies ###
# Install packaged dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
  build-essential git curl cmake gcc g++ pkg-config libmagic-dev \
  libssl-dev zlib1g-dev postgresql

### STEP 2: Create user ###
RUN adduser --home /home/cratesfyi --disabled-login --disabled-password --gecos "" cratesfyi

### STEP 3: Setup build environment as new user ###
ENV CRATESFYI_PREFIX="$CRATESFYI_PREFIX"
ENV CRATESFYI_DATABASE_URL=postgresql://cratesfyi:password@localhost
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
RUN cargo build --release

### STEP 5: Setup the database ###
RUN psql -c "CREATE USER cratesfyi WITH PASSWORD 'password';"
RUN psql -c "CREATE DATABASE cratesfyi OWNER cratesfyi;"

WORKDIR ~/docs.rs
RUN cargo run --release -- database init
RUN cargo run --release -- build add-essential-files
RUN cargo run --release -- build crate rand 0.5.5
RUN cargo run --release -- database update-search-index
RUN cargo run --release -- database update-release-activity

### STEP 6: Start the webserver ###
USER root
COPY setup/cratesfyi.server /etc/systemd/system/cratesfyi.service
RUN systemctl start cratesfyi.server
