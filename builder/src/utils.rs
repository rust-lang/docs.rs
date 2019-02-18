use crate::Result;
use failure::err_msg;
use regex::Regex;
use std::process::{Command, Output};

/// Gets rustc version.
pub fn get_rustc_version() -> Result<String> {
    Ok(
        command_result(Command::new("rustc").arg("--version").output()?)?
            .trim_end()
            .to_string(),
    )
}

/// Gets resouce suffix.
///
/// Resource suffix is generated from rustc version. Resource suffix for
/// `rustc 1.34.0-nightly (146aa60f3 2019-02-18)` will become `-20190218-1.34.0-nightly-146aa60f3`.
pub fn resource_suffix() -> Result<String> {
    let rustc_version = command_result(Command::new("rustc").arg("--version").output()?)?;
    parse_rustc_version(rustc_version)
}

/// Parses rustc commit hash from rustc version string and generate resource suffix.
fn parse_rustc_version<S: AsRef<str>>(version: S) -> Result<String> {
    let version_regex = Regex::new(r" ([\w.-]+) \((\w+) (\d+)-(\d+)-(\d+)\)")?;
    let captures = version_regex
        .captures(version.as_ref())
        .ok_or_else(|| err_msg("Failed to parse rustc version"))?;

    Ok(format!(
        "-{}{}{}-{}-{}",
        captures.get(3).unwrap().as_str(),
        captures.get(4).unwrap().as_str(),
        captures.get(5).unwrap().as_str(),
        captures.get(1).unwrap().as_str(),
        captures.get(2).unwrap().as_str()
    ))
}

fn command_result(output: Output) -> Result<String> {
    let mut command_out = String::from_utf8_lossy(&output.stdout).into_owned();
    command_out.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(command_out)
    } else {
        Err(err_msg(command_out))
    }
}

#[test]
fn test_parse_rustc_version() {
    assert_eq!(
        parse_rustc_version("rustc 1.34.0-nightly (146aa60f3 2019-02-18)").unwrap(),
        "-20190218-1.34.0-nightly-146aa60f3".to_string()
    );
}
