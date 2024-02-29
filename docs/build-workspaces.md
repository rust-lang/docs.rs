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

