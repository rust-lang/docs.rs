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
use askama::filters::Safe;
use chrono::{DateTime, NaiveDate, Utc};
use docs_rs_mimes as mimes;
use docs_rs_storage::FolderEntry;

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
/// generate this version of this crate, based on the rustc nightly date of the build.
pub(crate) fn get_correct_docsrs_style_file(date: NaiveDate) -> &'static str {
    const RUSTC_PR_144476_MERGED: NaiveDate = NaiveDate::from_ymd_opt(2025, 8, 20).unwrap();
    const RUSTC_PR_91356_MERGED: NaiveDate = NaiveDate::from_ymd_opt(2021, 12, 5).unwrap();

    if RUSTC_PR_144476_MERGED < date {
        "rustdoc-2025-08-20.css"
    } else if RUSTC_PR_91356_MERGED < date {
        // If this is the new rustdoc layout, we need the newer docs.rs CSS file.
        "rustdoc-2021-12-05.css"
    } else {
        // By default, we return the old docs.rs CSS file.
        "rustdoc.css"
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
    use crate::testing::{AxumRouterTestExt as _, TestEnvironmentExt as _, async_wrapper};
    use test_case::test_case;

    #[test_case(2016, 5, 23, "rustdoc.css")]
    #[test_case(2021, 12, 5, "rustdoc.css")]
    #[test_case(2021, 12, 6, "rustdoc-2021-12-05.css")]
    #[test_case(2025, 8, 20, "rustdoc-2021-12-05.css")]
    #[test_case(2025, 8, 21, "rustdoc-2025-08-20.css")]
    #[test_case(2999, 12, 29, "rustdoc-2025-08-20.css"; "synthetic CI toolchain")]
    fn test_get_correct_docsrs_style_file(y: i32, m: u32, d: u32, expected: &'static str) {
        let css_file = get_correct_docsrs_style_file(NaiveDate::from_ymd_opt(y, m, d).unwrap());
        assert_eq!(css_file, expected);

        // the file is actually served, at the URL `rustdoc/head.html` links to.
        async_wrapper(|env| async move {
            env.web_app()
                .await
                .assert_success(&format!(
                    "/-/static/v/{}/{css_file}",
                    crate::STATIC_ASSET_HASH
                ))
                .await?;
            Ok(())
        });
    }
}
