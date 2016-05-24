
# Crates.fyi

Centralized crate documentation builder and explorer for Rust programming
language. This program is using `lxc` containers to build documentation.


## Installation

Crates.fyi needs `cratesfyi-prefix` directory and a postgresql server to run.
This directory must have:

* Clone of `crates.io-index` repository.
* `sources` directory for crate source files.
* `cratesfyi-container`, a lxc container for building crates. This container
  must use exact same operating system as host machine to avoid conflicts
  (or you can build cratesfyi in guest system).
* `public_html/crates` directory for crate documentations.


An example script to create cratesfyi-prefix directory. Make sure you have
`git` and `lxc` packages installed. **Run this script as a normal user**:


```sh
#!/bin/sh
# Creates cratesfyi-prefix directory for cratesfyi
# This script is designed to run on Debian based operating systems,
# and tested under Debian jessie and sid

set -e

PREFIX=$(pwd)/cratesfyi-prefix
DIST_TEMPLATE=debian
DIST_RELEASE=jessie
DIST_MIRROR=http://httpredir.debian.org/debian

mkdir $PREFIX
mkdir -p $PREFIX/sources $PREFIX/public_html/crates
git clone https://github.com/rust-lang/crates.io-index.git $PREFIX/crates.io-index

# Create debian8 lxc container into cratesfyi-container directory
# Use your own distribution template and release name
sudo LANG=C MIRROR=$DIST_MIRROR \
    lxc-create -n cratesfyi-container -P $PREFIX \
    -t $DIST_TEMPLATE -- -r $DIST_RELEASE

# Due to some bug in lxc-attach this container
# must have a symbolic link in /var/lib/lxc
sudo ln -s $PREFIX/cratesfyi-container /var/lib/lxc

# Container directory must be accessible by current user
sudo chmod 755 $PREFIX/cratesfyi-container

# Setup lxc network
echo 'USE_LXC_BRIDGE="true"
LXC_BRIDGE="lxcbr0"
LXC_ADDR="10.0.3.1"
LXC_NETMASK="255.255.255.0"
LXC_NETWORK="10.0.3.0/24"
LXC_DHCP_RANGE="10.0.3.2,10.0.3.254"
LXC_DHCP_MAX="253"
LXC_DHCP_CONFILE=""
LXC_DOMAIN=""' | sudo tee /etc/default/lxc-net

# Start network interface
sudo service lxc-net restart

# Setup network for container
sudo sed -i 's/lxc.network.type.*/lxc.network.type = veth\nlxc.network.link = lxcbr0/' \
    $PREFIX/cratesfyi-container/config

# Start lxc container
sudo lxc-start -n cratesfyi-container

# Add user accounts into container
# Cratesfyi is using multiple user accounts to run cargo simultaneously
for user in $(whoami) cratesfyi updater; do
    sudo lxc-attach -n cratesfyi-container -- \
        adduser --disabled-login --disabled-password --gecos "" $user
done

# Install required packages for rust installation
sudo lxc-attach -n cratesfyi-container -- apt-get update
sudo lxc-attach -n cratesfyi-container -- apt-get install -y file git curl sudo ca-certificates

# Install rust nightly into container
sudo lxc-attach -n cratesfyi-container -- \
    su - -c 'curl -sSf https://static.rust-lang.org/rustup.sh | sh -s -- --channel=nightly'
```


And for the last you only need to install cratesfyi into guest machine (or build in guest
machine) to complete installation. If your host and guest operating system is
same simply build cratesfyi in release mode and copy into `/usr/local/bin`
directory of guest system:

```sh
cargo build --release
cp target/release/cratesfyi cratesfyi-prefix/rootfs/usr/local/bin/
```

Cratesfyi is only using `lxd-attach` command with sudo. Make sure your user
account can use this command without root password. Example `sudoers` entry:

```text
yourusername	ALL=(ALL) NOPASSWD: /usr/sbin/chroot
```


## Environment variables

cratesfyi is using few environment variables:

* `CRATESFYI_PREFIX` Prefix directory for cratesfyi
* `CRATESFYI_DATABASE_URL` Postgresql database URL
* `RUST_LOG` Set this to desired log level to get log messages
