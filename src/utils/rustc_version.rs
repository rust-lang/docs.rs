use crate::error::Result;
use anyhow::{Context, anyhow};
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

pub(crate) fn parse_rustc_date<S: AsRef<str>>(version: S) -> Result<NaiveDate> {
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

/// Picks the correct "rustdoc.css" static file depending on which rustdoc version was used to
/// generate this version of this crate.
pub fn get_correct_docsrs_style_file(version: &str) -> Result<String> {
    let date = parse_rustc_date(version)?;
    // This is the date where https://github.com/rust-lang/rust/pull/144476 was merged.
    if NaiveDate::from_ymd_opt(2025, 8, 20).unwrap() < date {
        Ok("rustdoc-2025-08-20.css".to_owned())
    // This is the date where https://github.com/rust-lang/rust/pull/91356 was merged.
    } else if NaiveDate::from_ymd_opt(2021, 12, 5).unwrap() < date {
        // If this is the new rustdoc layout, we need the newer docs.rs CSS file.
        Ok("rustdoc-2021-12-05.css".to_owned())
    } else {
        // By default, we return the old docs.rs CSS file.
        Ok("rustdoc.css".to_owned())
    }
}

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

#[test]
fn test_get_correct_docsrs_style_file() {
    assert_eq!(
        get_correct_docsrs_style_file("rustc 1.10.0-nightly (57ef01513 2016-05-23)").unwrap(),
        "rustdoc.css"
    );
    assert_eq!(
        get_correct_docsrs_style_file("docsrs 0.2.0 (ba9ae23 2022-05-26)").unwrap(),
        "rustdoc-2021-12-05.css"
    );
    assert!(get_correct_docsrs_style_file("docsrs 0.2.0").is_err(),);
}
