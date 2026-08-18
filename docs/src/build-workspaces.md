# Build Workspaces
## When do we need this?
Many workspace packages do not need manual intervention and can be built simply by executing the commands listed in the main [Readme.md](../Readme.md) file.
However, some workspaces require an additional step.
This is the case when values such as 
```toml
version.workspace = true
```
are inherited from the workspaces `Cargo.toml` configuration file.

## Fix
To build documentation, rustdoc requires a fully specified package but rustdoc does not understand workspaces which are only defined in cargo.
Thus our crate needs to be packaged by cargo before running the documentation.
This step will replace all of the `value.workspace = true` statements with their respective values.
```
cargo package
```
This will emit a packaged  crate into the `target/package/your_crate_name-version` folder.
Now the commands specified in [Readme.md](../Readme.md) can be executed targeting this folder.
```
cargo run -- build crate --local /path/to/source/target/package/your_crate_name-version/
```

## Full MWE
To showcase when such problems can occur, take a look at the following example.
### Structure
```bash
$ tree
.
├── Cargo.toml
├── my_lib
│   ├── Cargo.toml
│   └── src
│       └── lib.rs
└── README.md

3 directories, 4 files
```
The actual contents of `my_lib` do not matter, only the two configuration files.
```
$ cat Cargo.toml
[workspace]
members = [
        "my_lib",
]

[workspace.package]
version = "0.1.0"
```
and
```bash
$ cat my_lib/Cargo.toml
[package]
name = "my_lib"
version.workspace = true

[dependencies]
```

### Building

The build command
```bash
cargo run -- build crate -l path/to/docs_rs_workspace_package/my_lib
```
fails with
```bash
Error: Building documentation failed

Caused by:
    Building documentation failed

Caused by:
    invalid Cargo.toml syntax
```
which makes sense due to
```toml
version.workspace = true
```

### Fix
However when running the following sequence of commands
```bash
# Run this in the directory of docs_rs_workspace_package
cargo package -p my_lib
```
and then building again
```bash
# Run this from the docs.rs repo
cargo run -- build crate -l path/to/docs_rs_workspace_package/target/package/my_lib-0.1.0
```
then the build succeeds.

