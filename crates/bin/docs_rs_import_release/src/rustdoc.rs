use crate::common::{DOCS_RS, download};
use anyhow::{Result, bail};
use docs_rs_storage::AsyncStorage;
use docs_rs_utils::spawn_blocking;
use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use regex::Regex;
use std::{collections::HashSet, fmt, path::Path, sync::LazyLock};
use tokio::{
    fs,
    io::{self, AsyncBufReadExt as _},
    process::Command,
};
use tracing::debug;
use walkdir::WalkDir;

const KNOWN_STATIC_PATHS: &[&str] = &[
    "/-/rustdoc.static/FiraSans-Italic-81dc35de.woff2",
    "/-/rustdoc.static/FiraSans-Medium-e1aa3f0a.woff2",
    "/-/rustdoc.static/FiraSans-MediumItalic-ccf7e434.woff2",
    "/-/rustdoc.static/FiraSans-Regular-0fe48ade.woff2",
    "/-/rustdoc.static/SourceCodePro-Regular-8badfe75.ttf.woff2",
    "/-/rustdoc.static/SourceCodePro-Semibold-aa29a496.ttf.woff2",
    "/-/rustdoc.static/SourceSerif4-Regular-6b053e98.ttf.woff2",
];

pub(crate) async fn find_static_paths(
    root_dir: impl AsRef<Path> + fmt::Debug,
) -> Result<HashSet<String>> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r#""({}[^"]+)""#,
            docs_rs_utils::RUSTDOC_STATIC_PATH.replace(".", "\\.")
        ))
        .unwrap()
    });

    debug!("finding HTML files...");
    let root_dir = root_dir.as_ref();
    let html_files = spawn_blocking({
        let root_dir = root_dir.to_path_buf();
        move || {
            let mut files = Vec::new();
            for entry in WalkDir::new(&root_dir).follow_links(false).into_iter() {
                let entry = entry?;
                let path = entry.path();
                if entry.file_type().is_file()
                    && path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
                {
                    files.push(path.to_path_buf());
                }
            }
            Ok(files)
        }
    })
    .await?;

    debug!(
        count = html_files.len(),
        "finding static URLs in HTML files..."
    );
    const MAX_RUSTDOC_STATIC_FILE_COUNT: usize = 64;
    let mut urls = stream::iter(html_files)
        .map(|path| async move {
            let reader = io::BufReader::new(fs::File::open(&path).await?);
            let mut lines = reader.lines();

            let mut matches = HashSet::with_capacity(MAX_RUSTDOC_STATIC_FILE_COUNT);
            while let Some(line) = lines.next_line().await? {
                matches.extend(
                    RE.captures_iter(&line)
                        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string())),
                );
            }

            Ok::<_, anyhow::Error>(matches)
        })
        .buffer_unordered(16)
        .try_fold(
            HashSet::with_capacity(MAX_RUSTDOC_STATIC_FILE_COUNT),
            |mut urls, matches| async move {
                urls.extend(matches);
                Ok(urls)
            },
        )
        .await?;

    // these files aren't referenced directly in the HTML code, but their imports
    // are generated through JS.
    // Since these are statically known and barely change, I can just add them here.
    urls.remove("/-/rustdoc.static/${f}");
    urls.extend(KNOWN_STATIC_PATHS.iter().map(ToString::to_string));

    Ok(urls)
}

pub(crate) async fn download_static_files(
    storage: &AsyncStorage,
    paths: impl IntoIterator<Item = &str>,
) -> Result<()> {
    for path in paths {
        let key = format!(
            "{}{}",
            docs_rs_utils::RUSTDOC_STATIC_STORAGE_PREFIX,
            path.trim_start_matches(docs_rs_utils::RUSTDOC_STATIC_PATH)
        );

        if storage.exists(&key).await? {
            debug!("static file already exists in storage: {}", &key);
            continue;
        }

        storage
            .store_one(key, download(format!("{DOCS_RS}{path}")).await?)
            .await?;
    }

    Ok(())
}

pub(crate) async fn find_successful_build_targets(
    rustdoc_dir: impl AsRef<Path> + fmt::Debug,
    default_target: &str,
    other_targets: impl IntoIterator<Item = &str>,
) -> Result<Vec<String>> {
    let rustdoc_dir = rustdoc_dir.as_ref();

    let mut potential_other_targets: HashSet<String> =
        other_targets.into_iter().map(ToString::to_string).collect();
    potential_other_targets.extend(fetch_target_list().await?.into_iter());
    potential_other_targets.remove(default_target);

    let mut targets: Vec<String> = vec![default_target.into()];
    for t in potential_other_targets {
        if rustdoc_dir.join(&t).is_dir() {
            // non-default targets lead to a subdirectory in rustdoc
            targets.push(t);
        }
    }

    Ok(targets)
}

async fn fetch_target_list() -> Result<HashSet<String>> {
    let output = Command::new("rustc")
        .arg("--print")
        .arg("target-list")
        .output()
        .await?;

    if !output.status.success() {
        bail!(
            "`rustc --print target-list` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8(output.stdout)?;

    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}
