# Cratesfyi Command-line Arguments

```sh
$ cratesfyi --help

cratesfyi 0.6.0 (b667d28 2020-04-14)


USAGE:
    cratesfyi.exe <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    build               Builds documentation in a chroot environment
    daemon              Starts cratesfyi daemon
    database            Database operations
    help                Prints this message or the help of the given subcommand(s)
    queue               Interactions with the build queue
    start-web-server    Starts web server
```

```sh
$ cratesfyi build --help

cratesfyi.exe-build 0.6.0
Builds documentation in a chroot environment

USAGE:
    cratesfyi.exe build [FLAGS] [OPTIONS] --prefix <PREFIX> <SUBCOMMAND>

FLAGS:
    -k, --keep-build-directory    Keeps build directory after build
    -s, --skip                    Skips building documentation if documentation exists
        --skip-if-log-exists      Skips building documentation if build log exists
    -h, --help                    Prints help information
    -V, --version                 Prints version information

OPTIONS:
        --crates-io-index-path <CRATES_IO_INDEX_PATH>    Sets crates.io-index path
    -P, --prefix <PREFIX>                                 [env: CRATESFYI_PREFIX=ignored/cratesfyi-prefix]

SUBCOMMANDS:
    add-essential-files    Adds essential files for the installed version of rustc
    crate                  Builds documentation for a crate
    help                   Prints this message or the help of the given subcommand(s)
    lock                   Locks cratesfyi daemon to stop building new crates
    print-options
    unlock                 Unlocks cratesfyi daemon to continue building new crates
    update-toolchain       update the currently installed rustup toolchain
    world                  Builds documentation of every crate
```

<details>
<summary>Build Sub-Commands</summary>

```sh
$ cratesfyi build add-essential-files --help

cratesfyi.exe-build-add-essential-files 0.6.0
Adds essential files for the installed version of rustc

USAGE:
    cratesfyi.exe build --prefix <PREFIX> add-essential-files

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
```

```sh
$ cratesfyi build crate --help

cratesfyi.exe-build-crate 0.6.0
Builds documentation for a crate

USAGE:
    cratesfyi.exe build --prefix <PREFIX> crate [OPTIONS] <CRATE_NAME> <CRATE_VERSION>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -l, --local <local>    Build a crate at a specific path

ARGS:
    <CRATE_NAME>       Crate name
    <CRATE_VERSION>    Version of crate
```

```sh
$ cratesfyi build lock --help

cratesfyi.exe-build-lock 0.6.0
Locks cratesfyi daemon to stop building new crates

USAGE:
    cratesfyi.exe build --prefix <PREFIX> lock

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
```

```sh
$ cratesfyi build print-options --help

cratesfyi.exe-build-print-options 0.6.0

USAGE:
    cratesfyi.exe build --prefix <PREFIX> print-options

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
```

```sh
$ cratesfyi build unlock --help

cratesfyi.exe-build-unlock 0.6.0
Unlocks cratesfyi daemon to continue building new crates

USAGE:
    cratesfyi.exe build --prefix <PREFIX> unlock

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
```

```sh
$ cratesfyi build update-toolchain --help

cratesfyi.exe-build-update-toolchain 0.6.0
update the currently installed rustup toolchain

USAGE:
    cratesfyi.exe build --prefix <PREFIX> update-toolchain [FLAGS]

FLAGS:
        --only-first-time    Update the toolchain only if no toolchain is currently installed
    -h, --help               Prints help information
    -V, --version            Prints version information
```

```sh
$ cratesfyi build world --help

cratesfyi.exe-build-world 0.6.0
Builds documentation of every crate

USAGE:
    cratesfyi.exe build --prefix <PREFIX> world

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
```

</details>

```sh
$ cratesfyi daemon --help

cratesfyi.exe-daemon 0.6.0
Starts cratesfyi daemon

USAGE:
    cratesfyi.exe daemon [FLAGS]

FLAGS:
    -f, --foreground    Run the server in the foreground instead of detaching a child
    -h, --help          Prints help information
    -V, --version       Prints version information
```

```sh
$ cratesfyi database --help

cratesfyi.exe-database 0.6.0
Database operations

USAGE:
    cratesfyi.exe database <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    add-directory
    blacklist                  Blacklist operations
    delete-crate               Removes a whole crate from the database
    help                       Prints this message or the help of the given subcommand(s)
    migrate                    Run database migrations
    move-to-s3
    update-github-fields       Updates github stats for crates
    update-release-activity    Updates monthly release activity chart
    update-search-index        Updates search index
```

<details>
<summary>Database Sub-Commands</summary>

```sh
$ cratesfyi database add-directory --help

cratesfyi.exe-database-add-directory 0.6.0

USAGE:
    cratesfyi.exe database add-directory <DIRECTORY> <PREFIX>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

ARGS:
    <DIRECTORY>    Path of file or directory
    <PREFIX>       Prefix of files in database [env: CRATESFYI_PREFIX=ignored/cratesfyi-prefix]
```

```sh
$ cratesfyi database blacklist --help

cratesfyi.exe-database-blacklist 0.6.0
Blacklist operations

USAGE:
    cratesfyi.exe database blacklist <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    add       Add a crate to the blacklist
    help      Prints this message or the help of the given subcommand(s)
    list      List all crates on the blacklist
    remove    Remove a crate from the blacklist
```

<details>
<summary>Blacklist Sub-Commands</summary>

```sh
$ cratesfyi database blacklist add --help

cratesfyi.exe-database-blacklist-add 0.6.0
Add a crate to the blacklist

USAGE:
    cratesfyi.exe database blacklist add <CRATE_NAME>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

ARGS:
    <CRATE_NAME>    Crate name
```

```sh
$ cratesfyi database blacklist list --help

cratesfyi.exe-database-blacklist-list 0.6.0
List all crates on the blacklist

USAGE:
    cratesfyi.exe database blacklist list

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
```

```sh
$ cratesfyi database blacklist remove --help

cratesfyi.exe-database-blacklist-remove 0.6.0
Remove a crate from the blacklist

USAGE:
    cratesfyi.exe database blacklist remove <CRATE_NAME>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

ARGS:
    <CRATE_NAME>    Crate name
```

</details>

</details>

```sh
$ cratesfyi queue --help

cratesfyi.exe-queue 0.6.0
Interactions with the build queue

USAGE:
    cratesfyi.exe queue <SUBCOMMAND>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

SUBCOMMANDS:
    add     Add a crate to the build queue
    help    Prints this message or the help of the given subcommand(s)
```

<details>
<summary>Queue Sub-Commands</summary>

```sh
$ cratesfyi queue add --help

cratesfyi.exe-queue-add 0.6.0
Add a crate to the build queue

USAGE:
    cratesfyi.exe queue add [OPTIONS] <CRATE_NAME> <CRATE_VERSION>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -p, --priority <BUILD_PRIORITY>    Priority of build (new crate builds get priority 0) [default: 5]

ARGS:
    <CRATE_NAME>       Name of crate to build
    <CRATE_VERSION>    Version of crate to build
```

</details>

```sh 
$ cratesfyi start-web-server --help

cratesfyi.exe-start-web-server 0.6.0
Starts web server

USAGE:
    cratesfyi.exe start-web-server [SOCKET_ADDR]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

ARGS:
    <SOCKET_ADDR>     [default: 0.0.0.0:3000]
```
