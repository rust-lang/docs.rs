[Crates.fyi](https://crates.fyi/) - Documentation generator for crates
released in [crates.io](https://crates.io)

This program is an attempt to make a centralized documentation repository
for crates available in crates.io. Program is using chroot environment to
build documentation and replacing links on the fly.

All documentation is available in <https://crates.fyi/crates/>. There is also
an anonymous rsync server running on build machine. You can use rsync to
download everything. i.e:

```
rsync -a rsync://build.crates.fyi/crates crates
```

This command will download every crate documentation. If you want to download
documentation of a specific crate, you can use:

```
rsync -a rsync://build.crates.fyi/crates/<CRATE>/<VERSION> destination`
```

## Usage

```
./cratesfyi build [FLAGS] [OPTIONS] world
./cratesfyi build [FLAGS] [OPTIONS] crate <CRATE> <VERSION>
```

### Preparing chroot environment

This program is using a chroot environment to build documentation. I don't
think it was necessary but I didn't wanted to add bunch of stuff to my
stable server and a little bit more security doesn't hurt anyone.

chroot environment must be placed in **current\_working\_dir/chroot**
directory. And you must install desired version of rustc inside chroot
environment. Don't forget to add a regular user. This program is
using _onur_ username for chroot user for now.

You also need to clone crates.io-index respository. You can clone repository
from [crates.io-index](https://github.com/rust-lang/crates.io-index).

This program is using _sudo_ to use chroot. chroot is only command
used with sudo in this program. Make sure user has privileges to run chroot
command with sudo.

And lastly, you need to copy build.sh program into users home directory with
**.build.sh** name. Make sure chroot user has permissions to execute
**.build.sh** program.

Directory structure should look like this:

```text
.
├── cratesfyi                       # Main program (or cwd)
├── chroot                          # chroot environment
│   ├── bin
│   ├── etc
│   ├── home
│   │   └── onur                    # chroot user's home directory
│   │       └── .build.sh           # Build program to run cargo doc
│   └── ...
├── crates.io-index                 # Clone of crates.io-index
│   ├── 1
│   ├── 2
│   └── ...
├── logs                            # Build logs will be placed here
│   └── ...
└── public_html
    └── crates                      # Documentations will be placed here
```

chroot user's _home directory_ is called _build\_dir_ in program

### build subcommand arguments

Type `./cratesfyi build --help` to get full list of _FLAGS_ and _OPTIONS_.
