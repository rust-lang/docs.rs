
// web interface of crates.fyi
//
// Database layout:
//
// Crate:
// * id
// * name
// * latest-version
// * authors
// * licence
// * repository
// * documentation
// * homepage
// * description
// * github stars
// * github forks
// * badges (travis etc)
// * keywords
// * issues
//
// Releases:
// * id
// * crate_id
// * version
// * dependencies
// * yanked
// * rustdoc_status
// * test_status
//
//-------------------------------------------------------------------------
// TODO:
// * I need to get name, versions, deps, yanked from crates.io-index
// * Rest of the fields must be taken from crates Cargo.toml
// * Need to write a parser to parse eiter
//   lib.rs, main.rs to get long description of crate.
//   If long description is not available, need to get long description
//   from README.md.
// * Need to write a parser to get travis and other (?) badges.
// * Need to write a github client to get stars, forks and issues.
//
// Dev steps are basically follows:
// * Read and understand iron and how to use it with postresql.
// * Crate a postgresql database.
// * Start working on database module.
