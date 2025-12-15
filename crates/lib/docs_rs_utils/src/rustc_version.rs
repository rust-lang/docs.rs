use anyhow::{Context as _, Result, anyhow};
use chrono::prelude::*;
use regex::Regex;
use std::sync::LazyLock;

/// Parses rustc commit hash from rustc version string
pub fn parse_rustc_version<S: AsRef<str>>(version: S) -> Result<String> {
    let version_regex = Regex::new(r" ([\w.-]+) \((\w+) (\d+)-(\d+)-(\d+)\)")?;
    let captures = version_regex
        .captures(version.as_ref())
        .with_context(|| anyhow!("Failed to parse rustc version '{}'", version.as_ref()))?;

    Ok(format!(
        "{}{}{}-{}-{}",
        captures.get(3).unwrap().as_str(),
        captures.get(4).unwrap().as_str(),
        captures.get(5).unwrap().as_str(),
        captures.get(1).unwrap().as_str(),
        captures.get(2).unwrap().as_str()
    ))
}

pub fn parse_rustc_date<S: AsRef<str>>(version: S) -> Result<NaiveDate> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" (\d+)-(\d+)-(\d+)\)$").unwrap());

    let cap = RE
        .captures(version.as_ref())
        .with_context(|| anyhow!("Failed to parse rustc date"))?;

    let year = cap.get(1).unwrap().as_str();
    let month = cap.get(2).unwrap().as_str();
    let day = cap.get(3).unwrap().as_str();

    NaiveDate::from_ymd_opt(
        year.parse::<i32>().unwrap(),
        month.parse::<u32>().unwrap(),
        day.parse::<u32>().unwrap(),
    )
    .ok_or_else(|| anyhow!("date out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rustc_version() {
        assert_eq!(
            parse_rustc_version("rustc 1.10.0-nightly (57ef01513 2016-05-23)").unwrap(),
            "20160523-1.10.0-nightly-57ef01513"
        );
        assert_eq!(
            parse_rustc_version("docsrs 0.2.0 (ba9ae23 2016-05-26)").unwrap(),
            "20160526-0.2.0-ba9ae23"
        );
    }
}
