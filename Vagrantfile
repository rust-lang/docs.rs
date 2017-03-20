# -*- mode: ruby -*-
# vi: set ft=ruby :

Vagrant.configure("2") do |config|
  config.vm.box = "docs.rs"
  config.vm.box_url = "https://docs.rs/vagrant.box"
  config.vm.box_download_checksum = "02554eb16ac72367980a883c4ff05f17d9988406"
  config.vm.box_download_checksum_type = "sha1"

  config.ssh.username = "cratesfyi"

  config.vm.network "forwarded_port", guest: 3000, host: 3000

  # Use 25% of available system memory and all CPU's
  config.vm.provider "virtualbox" do |vb|
    host = RbConfig::CONFIG['host_os']

    if host =~ /darwin/
      cpus = `sysctl -n hw.ncpu`.to_i
      mem = `sysctl -n hw.memsize`.to_i / 1024 / 1024 / 4
    elsif host =~ /linux/
      cpus = `nproc`.to_i
      mem = `grep 'MemTotal' /proc/meminfo | sed -e 's/MemTotal://' -e 's/ kB//'`.to_i / 1024 / 4
    else
      cpus = 2
      mem = 1024
    end

    vb.memory = mem
    vb.cpus = cpus
  end


  # docs.rs vagrant image comes with only a pre-configured cratesfyi-container
  # installing rest with provision
  config.vm.provision "shell", inline: <<-SHELL
    set -ev

    ############################################################
    # Installing docs.rs dependencies                          #
    ############################################################
    apt-get update
    apt-get install -y --no-install-recommends cmake curl cmake gcc g++ git libmagic-dev libssl-dev pkg-config

    ############################################################
    # Installing rustc into cratesfyi-container                #
    ############################################################
    lxc-attach -n cratesfyi-container -- apt-get update
    lxc-attach -n cratesfyi-container -- apt-get install -y --no-install-recommends curl ca-certificates binutils gcc libc6-dev libmagic1
    lxc-attach -n cratesfyi-container -- su - cratesfyi -c 'curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain nightly-2017-03-19'

    ############################################################
    # Creating rustc links for cratesfyi user                  #
    ############################################################
    for directory in .cargo .rustup .multirust; do
      [[ -h /home/cratesfyi/$directory ]] || sudo -u cratesfyi ln -vs /var/lib/lxc/cratesfyi-container/rootfs/home/cratesfyi/$directory /home/cratesfyi/
    done

    ############################################################
    # Setting up environment variables                         #
    ############################################################
    [[ -f /home/cratesfyi/.cratesfyi.env ]] || sudo -u cratesfyi tee /home/cratesfyi/.cratesfyi.env <<\EOF
CRATESFYI_PREFIX=/home/cratesfyi/cratesfyi-prefix
CRATESFYI_DATABASE_URL=postgresql://cratesfyi@localhost
CRATESFYI_GITHUB_USERNAME=
CRATESFYI_GITHUB_ACCESSTOKEN=
RUST_LOG=cratesfyi
EOF

    ############################################################
    # Loading environment variables                            #
    ############################################################
    source /home/cratesfyi/.cratesfyi.env

    ############################################################
    # Preparing cratesfyi-prefix                               #
    ############################################################
    sudo -u cratesfyi mkdir -vp /home/cratesfyi/cratesfyi-prefix/documentations \
                                /home/cratesfyi/cratesfyi-prefix/public_html

    ############################################################
    # Getting external css files from docs.rs                  #
    ############################################################
    sudo -u cratesfyi wget -qcP /home/cratesfyi/cratesfyi-prefix/public_html \
                                https://docs.rs/rustdoc-20160526-1.10.0-nightly-97e3a2401.css \
                                https://docs.rs/main-20160526-1.10.0-nightly-97e3a2401.css

    ############################################################
    # Cloning crates.io-index                                  #
    ############################################################
    if [ ! -d /home/cratesfyi/cratesfyi-prefix/crates.io-index ]; then
        sudo -u cratesfyi git clone https://github.com/rust-lang/crates.io-index /home/cratesfyi/cratesfyi-prefix/crates.io-index
    else
        sudo -u cratesfyi git --git-dir=/home/cratesfyi/cratesfyi-prefix/crates.io-index/.git pull
    fi

    # Create `crates-index-diff_last-seen` branch for tracking new crates
    sudo -u cratesfyi git --git-dir=/home/cratesfyi/cratesfyi-prefix/crates.io-index/.git branch crates-index-diff_last-seen || true

    ############################################################
    # Building docs.rs                                         #
    ############################################################
    su - cratesfyi -c "cd /vagrant && cargo build" 2>&1

    ############################################################
    # Copying docs.rs into container                           #
    ############################################################
    cp -v /vagrant/target/debug/cratesfyi /var/lib/lxc/cratesfyi-container/rootfs/usr/local/bin

    ############################################################
    # Re-creating database                                     #
    ############################################################
    echo 'DROP DATABASE cratesfyi; CREATE DATABASE cratesfyi OWNER cratesfyi' | sudo -u postgres psql

    ############################################################
    # Initializing database scheme                             #
    ############################################################
    su - cratesfyi -c "cd /vagrant && cargo run -- database init"

    ############################################################
    # Add essential files for downloaded nigthly               #
    ############################################################
    su - cratesfyi -c "cd /vagrant && cargo run -- build add-essential-files" 2>&1

    ############################################################
    # Populating database by building some crates              #
    ############################################################
    su - cratesfyi -c "cd /vagrant && cargo run -- build crate rand 0.3.15" 2>&1
    su - cratesfyi -c "cd /vagrant && cargo run -- build crate log 0.3.6" 2>&1
    su - cratesfyi -c "cd /vagrant && cargo run -- build crate regex 0.1.80" 2>&1

    ############################################################
    # Update search index and release activity                 #
    ############################################################
    su - cratesfyi -c "cd /vagrant && cargo run -- database update-search-index" 2>&1
    su - cratesfyi -c "cd /vagrant && cargo run -- database update-release-activity" 2>&1


    ############################################################
    # docs.rs vagrant box is ready!                            #
    #----------------------------------------------------------#
    # You can connect to virtual machine with `vagrant ssh`.   #
    # docs.rs is available in `/vagrant` folder!               #
    ############################################################
  SHELL
end
