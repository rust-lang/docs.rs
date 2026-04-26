pub(crate) mod highlight;
pub(crate) mod html_rewrite;
pub(crate) mod licenses;
pub(crate) mod markdown;

use crate::{
    icons::{
        IconFile, IconFileLines, IconFolderOpen, IconGitAlt, IconLock, IconMarkdown, IconRust,
    },
    page::templates::{RenderBrands, RenderRegular, RenderSolid},
};
use anyhow::Result;
use askama::filters::Safe;
use chrono::{DateTime, NaiveDate, Utc};
use docs_rs_mimes as mimes;
use docs_rs_storage::FolderEntry;
use docs_rs_utils::rustc_version::parse_rustc_date;

pub fn folder_entry_icon(entry: &FolderEntry) -> Safe<String> {
    if entry.is_dir() {
        return IconFolderOpen.render_regular(false, false, "");
    }

    let mime = entry.mime().expect("files always have mime");
    let name = entry.name();

    if *mime == *mimes::TEXT_RUST {
        IconRust.render_brands(false, false, "")
    } else if *mime == mime::TEXT_PLAIN && name == "Cargo.lock" {
        IconLock.render_solid(false, false, "")
    } else if *mime == *mimes::TEXT_MARKDOWN {
        IconMarkdown.render_brands(false, false, "")
    } else if *mime == mime::TEXT_PLAIN && name == ".gitignore" {
        IconGitAlt.render_brands(false, false, "")
    } else if *mime == mime::TEXT_PLAIN || mime.type_() == "text" {
        IconFileLines.render_regular(false, false, "")
    } else {
        IconFile.render_regular(false, false, "")
    }
}

/// Picks the correct "rustdoc.css" static file depending on which rustdoc version was used to
/// generate this version of this crate.
pub(crate) fn get_correct_docsrs_style_file(version: &str) -> Result<String> {
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

/// Converts Timespec to nice readable relative time string
pub(crate) fn duration_to_str(init: DateTime<Utc>) -> String {
    let now = Utc::now();
    let delta = now.signed_duration_since(init);

    let delta = (
        delta.num_days(),
        delta.num_hours(),
        delta.num_minutes(),
        delta.num_seconds(),
    );

    match delta {
        (days, ..) if days > 5 => format!("{}", init.format("%b %d, %Y")),
        (days @ 2..=5, ..) => format!("{days} days ago"),
        (1, ..) => "one day ago".to_string(),

        (_, hours, ..) if hours > 1 => format!("{hours} hours ago"),
        (_, 1, ..) => "an hour ago".to_string(),

        (_, _, minutes, _) if minutes > 1 => format!("{minutes} minutes ago"),
        (_, _, 1, _) => "one minute ago".to_string(),

        (_, _, _, seconds) if seconds > 0 => format!("{seconds} seconds ago"),
        _ => "just now".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
