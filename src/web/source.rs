use super::{error::AxumResult, match_version};
use crate::{
    db::Pool,
    impl_axum_webpage,
    storage::PathNotFoundError,
    web::{
        cache::CachePolicy, error::AxumNope, extractors::Path, file::File as DbFile,
        headers::CanonicalUrl, MetaData, ReqVersion,
    },
    AsyncStorage,
};
use anyhow::{Context as _, Result};
use axum::{response::IntoResponse, Extension};
use axum_extra::headers::HeaderMapExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, sync::Arc};
use tracing::instrument;

/// A source file's name and mime type
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Serialize)]
struct File {
    /// The name of the file
    name: String,
    /// The mime type of the file
    mime: String,
}

impl File {
    fn from_path_and_mime(path: &str, mime: &str) -> File {
        let (name, mime) = if let Some((dir, _)) = path.split_once('/') {
            (dir, "dir")
        } else {
            (path, mime)
        };

        Self {
            name: name.to_owned(),
            mime: mime.to_owned(),
        }
    }
}

/// A list of source files
#[derive(Debug, Clone, PartialEq, Serialize)]
struct FileList {
    metadata: MetaData,
    files: Vec<File>,
}

impl FileList {
    /// Gets FileList from a request path
    ///
    /// All paths stored in database have this format:
    ///
    /// ```text
    /// [
    ///   ["text/plain", ".gitignore"],
    ///   ["text/x-c", "src/reseeding.rs"],
    ///   ["text/x-c", "src/lib.rs"],
    ///   ["text/x-c", "README.md"],
    ///   ...
    /// ]
    /// ```
    ///
    /// This function is only returning FileList for requested directory. If is empty,
    /// it will return list of files (and dirs) for root directory. req_path must be a
    /// directory or empty for root directory.
    #[instrument(skip(conn))]
    async fn from_path(
        conn: &mut sqlx::PgConnection,
        name: &str,
        version: &Version,
        req_version: Option<ReqVersion>,
        folder: &str,
    ) -> Result<Option<FileList>> {
        let row = match sqlx::query!(
            "SELECT releases.files
            FROM releases
            INNER JOIN crates ON crates.id = releases.crate_id
            WHERE crates.name = $1 AND releases.version = $2",
            name,
            version.to_string(),
        )
        .fetch_optional(&mut *conn)
        .await?
        {
            Some(row) => row,
            None => return Ok(None),
        };

        let files = if let Some(files) = row.files {
            files
        } else {
            return Ok(None);
        };

        let mut file_list = Vec::new();
        if let Some(files) = files.as_array() {
            file_list.reserve(files.len());

            for file in files {
                if let Some(file) = file.as_array() {
                    let mime = file[0].as_str().unwrap();
                    let path = file[1].as_str().unwrap();

                    // skip .cargo-ok generated by cargo
                    if path == ".cargo-ok" {
                        continue;
                    }

                    // look only files for req_path
                    if let Some(path) = path.strip_prefix(folder) {
                        let file = File::from_path_and_mime(path, mime);

                        // avoid adding duplicates, a directory may occur more than once
                        if !file_list.contains(&file) {
                            file_list.push(file);
                        }
                    }
                }
            }

            if file_list.is_empty() {
                return Ok(None);
            }

            file_list.sort_by(|a, b| {
                // directories must be listed first
                if a.mime == "dir" && b.mime != "dir" {
                    Ordering::Less
                } else if a.mime != "dir" && b.mime == "dir" {
                    Ordering::Greater
                } else {
                    a.name.to_lowercase().cmp(&b.name.to_lowercase())
                }
            });

            Ok(Some(FileList {
                metadata: MetaData::from_crate(conn, name, version, req_version).await?,
                files: file_list,
            }))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SourcePage {
    file_list: FileList,
    show_parent_link: bool,
    file: Option<File>,
    file_content: Option<String>,
    canonical_url: CanonicalUrl,
    is_file_too_large: bool,
    is_latest_url: bool,
    use_direct_platform_links: bool,
}

impl_axum_webpage! {
    SourcePage = "crate/source.html",
    canonical_url = |page| Some(page.canonical_url.clone()),
    cache_policy = |page| if page.is_latest_url {
        CachePolicy::ForeverInCdn
    } else {
        CachePolicy::ForeverInCdnAndStaleInBrowser
    },
    cpu_intensive_rendering = true,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct SourceBrowserHandlerParams {
    name: String,
    version: ReqVersion,
    #[serde(default)]
    path: String,
}

#[instrument(skip(pool, storage))]
pub(crate) async fn source_browser_handler(
    Path(params): Path<SourceBrowserHandlerParams>,
    Extension(storage): Extension<Arc<AsyncStorage>>,
    Extension(pool): Extension<Pool>,
) -> AxumResult<impl IntoResponse> {
    let mut conn = pool.get_async().await?;

    let version = match_version(&mut conn, &params.name, &params.version)
        .await?
        .into_exactly_named_or_else(|corrected_name, req_version| {
            AxumNope::Redirect(
                format!(
                    "/crate/{corrected_name}/{req_version}/source/{}",
                    params.path
                ),
                CachePolicy::NoCaching,
            )
        })?
        .into_canonical_req_version_or_else(|version| {
            AxumNope::Redirect(
                format!("/crate/{}/{version}/source/{}", params.name, params.path),
                CachePolicy::ForeverInCdn,
            )
        })?
        .into_version();

    let row = sqlx::query!(
        "SELECT
            releases.archive_storage,
            (
                SELECT id
                FROM builds
                WHERE builds.rid = releases.id
                ORDER BY build_time DESC
                LIMIT 1
            ) AS latest_build_id
         FROM releases
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE
             name = $1 AND
             version = $2",
        params.name,
        version.to_string()
    )
    .fetch_one(&mut *conn)
    .await?;

    // try to get actual file first
    // skip if request is a directory
    let (blob, is_file_too_large) = if !params.path.ends_with('/') {
        match storage
            .fetch_source_file(
                &params.name,
                &version.to_string(),
                row.latest_build_id.unwrap_or(0),
                &params.path,
                row.archive_storage,
            )
            .await
            .context("error fetching source file")
        {
            Ok(blob) => (Some(blob), false),
            Err(err) => match err {
                err if err.is::<PathNotFoundError>() => (None, false),
                // if file is too large, set is_file_too_large to true
                err if err.downcast_ref::<std::io::Error>().is_some_and(|err| {
                    err.get_ref()
                        .map(|err| err.is::<crate::error::SizeLimitReached>())
                        .unwrap_or(false)
                }) =>
                {
                    (None, true)
                }
                _ => return Err(err.into()),
            },
        }
    } else {
        (None, false)
    };

    let canonical_url = CanonicalUrl::from_path(format!(
        "/crate/{}/latest/source/{}",
        params.name, params.path
    ));

    let (file, file_content) = if let Some(blob) = blob {
        let is_text = blob.mime.starts_with("text") || blob.mime == "application/json";
        // serve the file with DatabaseFileHandler if file isn't text and not empty
        if !is_text && !blob.is_empty() {
            let mut response = DbFile(blob).into_response();
            response.headers_mut().typed_insert(canonical_url);
            response
                .extensions_mut()
                .insert(CachePolicy::ForeverInCdnAndStaleInBrowser);
            return Ok(response);
        } else if is_text && !blob.is_empty() {
            let path = blob
                .path
                .rsplit_once('/')
                .map(|(_, path)| path)
                .unwrap_or(&blob.path);
            (
                Some(File::from_path_and_mime(path, &blob.mime)),
                String::from_utf8(blob.content).ok(),
            )
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let current_folder = if let Some(last_slash_pos) = params.path.rfind('/') {
        &params.path[..last_slash_pos + 1]
    } else {
        ""
    };

    let file_list = FileList::from_path(
        &mut conn,
        &params.name,
        &version,
        Some(params.version.clone()),
        current_folder,
    )
    .await?
    .ok_or(AxumNope::ResourceNotFound)?;

    Ok(SourcePage {
        file_list,
        show_parent_link: !current_folder.is_empty(),
        file,
        file_content,
        canonical_url,
        is_file_too_large,
        is_latest_url: params.version.is_latest(),
        use_direct_platform_links: true,
    }
    .into_response())
}

#[cfg(test)]
mod tests {
    use crate::test::*;
    use crate::web::cache::CachePolicy;
    use kuchikiki::traits::TendrilSink;
    use reqwest::StatusCode;
    use test_case::test_case;

    fn get_file_list_links(body: &str) -> Vec<String> {
        let dom = kuchikiki::parse_html().one(body);

        dom.select(".package-menu > ul > li > a")
            .expect("invalid selector")
            .map(|el| {
                let attributes = el.attributes.borrow();
                attributes.get("href").unwrap().to_string()
            })
            .collect()
    }

    #[test_case(true)]
    #[test_case(false)]
    fn fetch_source_file_utf8_path(archive_storage: bool) {
        wrapper(|env| {
            let filename = "序.pdf";

            env.fake_release()
                .archive_storage(archive_storage)
                .name("fake")
                .version("0.1.0")
                .source_file(filename, b"some_random_content")
                .create()?;

            let web = env.frontend();
            let response = web
                .get(&format!("/crate/fake/0.1.0/source/{filename}"))
                .send()?;
            assert!(response.status().is_success());
            assert_eq!(
                response.headers().get("link").unwrap(),
                "<https://docs.rs/crate/fake/latest/source/%E5%BA%8F.pdf>; rel=\"canonical\"",
            );
            assert!(response.text()?.contains("some_random_content"));
            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn fetch_source_file_content(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(archive_storage)
                .name("fake")
                .version("0.1.0")
                .source_file("some_filename.rs", b"some_random_content")
                .create()?;
            let web = env.frontend();
            assert_success_cached(
                "/crate/fake/0.1.0/source/",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            let response = web
                .get("/crate/fake/0.1.0/source/some_filename.rs")
                .send()?;
            assert!(response.status().is_success());
            assert_eq!(
                response.headers().get("link").unwrap(),
                "<https://docs.rs/crate/fake/latest/source/some_filename.rs>; rel=\"canonical\""
            );
            assert_cache_control(
                &response,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            );
            assert!(response.text()?.contains("some_random_content"));
            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn fetch_binary(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(archive_storage)
                .name("fake")
                .version("0.1.0")
                .source_file("some_file.pdf", b"some_random_content")
                .create()?;
            let web = env.frontend();
            let response = web.get("/crate/fake/0.1.0/source/some_file.pdf").send()?;
            assert!(response.status().is_success());
            assert_eq!(
                response.headers().get("link").unwrap(),
                "<https://docs.rs/crate/fake/latest/source/some_file.pdf>; rel=\"canonical\""
            );
            assert_eq!(
                response
                    .headers()
                    .get("content-type")
                    .unwrap()
                    .to_str()
                    .unwrap(),
                "application/pdf"
            );

            assert_cache_control(
                &response,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            );
            assert!(response.text()?.contains("some_random_content"));
            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn cargo_ok_not_skipped(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(archive_storage)
                .name("fake")
                .version("0.1.0")
                .source_file(".cargo-ok", b"ok")
                .source_file("README.md", b"hello")
                .create()?;
            let web = env.frontend();
            assert_success("/crate/fake/0.1.0/source/", web)?;
            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn empty_file_list_dont_break_the_view(archive_storage: bool) {
        wrapper(|env| {
            let release_id = env
                .fake_release()
                .archive_storage(archive_storage)
                .name("fake")
                .version("0.1.0")
                .source_file("README.md", b"hello")
                .create()?;

            let path = "/crate/fake/0.1.0/source/README.md";
            let web = env.frontend();
            assert_success(path, web)?;

            env.db().conn().execute(
                "UPDATE releases
                     SET files = NULL
                     WHERE id = $1",
                &[&release_id],
            )?;

            assert_eq!(web.get(path).send()?.status(), StatusCode::NOT_FOUND);

            Ok(())
        });
    }

    #[test]
    fn latest_contains_links_to_latest() {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(true)
                .name("fake")
                .version("0.1.0")
                .source_file(".cargo-ok", b"ok")
                .source_file("README.md", b"hello")
                .create()?;
            let resp = env.frontend().get("/crate/fake/latest/source/").send()?;
            assert_cache_control(&resp, CachePolicy::ForeverInCdn, &env.config());
            assert!(resp.url().as_str().ends_with("/crate/fake/latest/source/"));
            let body = String::from_utf8(resp.bytes().unwrap().to_vec()).unwrap();
            assert!(body.contains("<a href=\"/crate/fake/latest/builds\""));
            assert!(body.contains("<a href=\"/crate/fake/latest/source/\""));
            assert!(body.contains("<a href=\"/crate/fake/latest\""));
            assert!(body.contains("<a href=\"/crate/fake/latest/features\""));

            Ok(())
        });
    }

    #[test_case(true)]
    #[test_case(false)]
    fn directory_not_found(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(archive_storage)
                .name("mbedtls")
                .version("0.2.0")
                .create()?;
            let web = env.frontend();
            assert_not_found("/crate/mbedtls/0.2.0/source/test/", web)?;
            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn semver_handled_latest(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(archive_storage)
                .name("mbedtls")
                .version("0.2.0")
                .source_file("README.md", b"hello")
                .create()?;
            let web = env.frontend();
            assert_success("/crate/mbedtls/0.2.0/source/", web)?;
            assert_redirect_cached(
                "/crate/mbedtls/*/source/",
                "/crate/mbedtls/latest/source/",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;
            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn semver_handled(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(archive_storage)
                .name("mbedtls")
                .version("0.2.0")
                .source_file("README.md", b"hello")
                .create()?;
            let web = env.frontend();
            assert_success("/crate/mbedtls/0.2.0/source/", web)?;
            assert_redirect_cached(
                "/crate/mbedtls/~0.2.0/source/",
                "/crate/mbedtls/0.2.0/source/",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;
            Ok(())
        })
    }

    #[test_case(true)]
    #[test_case(false)]
    fn literal_krate_description(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(archive_storage)
                .name("rustc-ap-syntax")
                .version("178.0.0")
                .description("some stuff with krate")
                .source_file("fold.rs", b"fn foo() {}")
                .create()?;
            let web = env.frontend();
            assert_success_cached(
                "/crate/rustc-ap-syntax/178.0.0/source/fold.rs",
                web,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            )?;
            Ok(())
        })
    }

    #[test]
    fn cargo_special_filetypes_are_highlighted() {
        wrapper(|env| {
            env.fake_release()
                .name("fake")
                .version("0.1.0")
                .source_file("Cargo.toml.orig", b"[package]")
                .source_file("Cargo.lock", b"[dependencies]")
                .create()?;

            let web = env.frontend();

            let response = web
                .get("/crate/fake/0.1.0/source/Cargo.toml.orig")
                .send()?
                .text()?;
            assert!(response.contains(r#"<span class="syntax-source syntax-toml">"#));

            let response = web
                .get("/crate/fake/0.1.0/source/Cargo.lock")
                .send()?
                .text()?;
            assert!(response.contains(r#"<span class="syntax-source syntax-toml">"#));

            Ok(())
        });
    }

    #[test]
    fn dotfiles_with_extension_are_highlighted() {
        wrapper(|env| {
            env.fake_release()
                .name("fake")
                .version("0.1.0")
                .source_file(".rustfmt.toml", b"[rustfmt]")
                .create()?;

            let web = env.frontend();

            let response = web
                .get("/crate/fake/0.1.0/source/.rustfmt.toml")
                .send()?
                .text()?;
            assert!(response.contains(r#"<span class="syntax-source syntax-toml">"#));

            Ok(())
        });
    }

    #[test]
    fn json_is_served_as_rendered_html() {
        wrapper(|env| {
            env.fake_release()
                .name("fake")
                .version("0.1.0")
                .source_file("Cargo.toml", b"")
                .source_file("config.json", b"{}")
                .create()?;

            let web = env.frontend();

            let response = web.get("/crate/fake/0.1.0/source/config.json").send()?;
            assert!(response
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/html"));

            let text = response.text()?;
            assert!(text.starts_with(r#"<!DOCTYPE html>"#));

            // file list doesn't show "../"
            assert_eq!(
                get_file_list_links(&text),
                vec!["./Cargo.toml", "./config.json"]
            );

            Ok(())
        });
    }

    #[test]
    fn root_file_list() {
        wrapper(|env| {
            env.fake_release()
                .name("fake")
                .version("0.1.0")
                .source_file("Cargo.toml", b"some_random_content")
                .source_file("folder1/some_filename.rs", b"some_random_content")
                .source_file("folder2/another_filename.rs", b"some_random_content")
                .source_file("root_filename.rs", b"some_random_content")
                .create()?;

            let web = env.frontend();
            let response = web.get("/crate/fake/0.1.0/source/").send()?;
            assert!(response.status().is_success());
            assert_cache_control(
                &response,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            );

            assert_eq!(
                get_file_list_links(&response.text()?),
                vec![
                    "./folder1/",
                    "./folder2/",
                    "./Cargo.toml",
                    "./root_filename.rs"
                ]
            );
            Ok(())
        });
    }

    #[test]
    fn child_file_list() {
        wrapper(|env| {
            env.fake_release()
                .name("fake")
                .version("0.1.0")
                .source_file("folder1/some_filename.rs", b"some_random_content")
                .source_file("folder1/more_filenames.rs", b"some_random_content")
                .source_file("folder2/another_filename.rs", b"some_random_content")
                .source_file("root_filename.rs", b"some_random_content")
                .create()?;

            let web = env.frontend();
            let response = web
                .get("/crate/fake/0.1.0/source/folder1/some_filename.rs")
                .send()?;
            assert!(response.status().is_success());
            assert_cache_control(
                &response,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            );

            assert_eq!(
                get_file_list_links(&response.text()?),
                vec!["../", "./more_filenames.rs", "./some_filename.rs"],
            );
            Ok(())
        });
    }
}
