
use std::process::{Command, Output};
use regex::Regex;
use error::Result;

/// Parses rustc commit hash from rustc version string
pub fn parse_rustc_version<S: AsRef<str>>(version: S) -> String {
    let version_regex = Regex::new(r" ([\w-.]+) \((\w+) (\d+)-(\d+)-(\d+)\)").unwrap();
    let captures = version_regex.captures(version.as_ref()).expect("Failed to parse rustc version");

    format!("{}{}{}-{}-{}",
            captures.get(3).unwrap().as_str(),
            captures.get(4).unwrap().as_str(),
            captures.get(5).unwrap().as_str(),
            captures.get(1).unwrap().as_str(),
            captures.get(2).unwrap().as_str())
}


/// Returns current version of rustc and cratesfyi
pub fn get_current_versions() -> Result<(String, String)> {
    let rustc_version = command_result(Command::new("rustc").arg("--version").output()?)?;
    let cratesfyi_version = command_result(Command::new("rustc").arg("--version").output()?)?;

    Ok((rustc_version, cratesfyi_version))
}


pub fn command_result(output: Output) -> Result<String> {
    let mut command_out = String::from_utf8_lossy(&output.stdout).into_owned();
    command_out.push_str(&String::from_utf8_lossy(&output.stderr).into_owned()[..]);
    match output.status.success() {
        true => Ok(command_out),
        false => Err(command_out.into()),
    }
}
