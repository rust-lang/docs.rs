use crate::{
    cache::CachePolicy,
    error::AxumResult,
    extractors::{
        DbConnection,
        rustdoc::{PageKind, RustdocParams},
    },
    handlers::axum_cached_redirect,
    match_release::match_version,
};

use axum::response::IntoResponse;
use tracing::instrument;

#[instrument(skip(conn))]
pub(crate) async fn source_browser_handler(
    params: RustdocParams,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    let params = params.with_page_kind(PageKind::Source);
    let matched_release = match_version(&mut conn, params.name(), params.req_version())
        .await?
        .into_exactly_named()
        .into_canonical_req_version();
    let params = params.apply_matched_release(&matched_release);

    Ok(axum_cached_redirect(
        params.source_url(),
        if params.req_version().is_latest() {
            CachePolicy::ForeverInCdn(params.name().into())
        } else {
            CachePolicy::ForeverInCdnAndStaleInBrowser(params.name().into())
        },
    )?)
}

#[cfg(test)]
mod tests {
    use crate::{
        cache::CachePolicy,
        testing::{AxumRouterTestExt, TestEnvironmentExt as _, async_wrapper},
    };
    use docs_rs_types::testing::{KRATE, V1, V2};
    use test_case::test_case;

    #[test_case("src/something.rs", "src/something.rs")]
    #[test_case("a/序.pdf", "a/%E5%BA%8F.pdf")]
    fn legacy_source_redirects_to_crates_io(filename: &'static str, expected: &'static str) {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .archive_storage(true)
                .name(&KRATE)
                .version(V1)
                .source_file(filename, b"some_random_content")
                .create()
                .await?;

            let web = env.web_app().await;

            // source root redirect
            web.assert_redirect_cached_unchecked(
                &format!("/crate/{KRATE}/{V1}/source/"),
                &format!("https://crates.io/crates/{KRATE}/{V1}/code/"),
                CachePolicy::ForeverInCdnAndStaleInBrowser(KRATE.into()),
                env.config(),
            )
            .await?;

            // source path redirect
            web.assert_redirect_cached_unchecked(
                &format!("/crate/{KRATE}/{V1}/source/{filename}"),
                &format!("https://crates.io/crates/{KRATE}/{V1}/code/{expected}"),
                CachePolicy::ForeverInCdnAndStaleInBrowser(KRATE.into()),
                env.config(),
            )
            .await?;

            Ok(())
        });
    }

    #[test_case("*", "2.0.0")]
    #[test_case("latest", "2.0.0")]
    fn latest_handled(req_version: &'static str, crates_io_version: &'static str) {
        async_wrapper(|env| async move {
            for v in [V1, V2] {
                env.fake_release()
                    .await
                    .archive_storage(true)
                    .name(KRATE)
                    .version(v)
                    .source_file("README.md", b"hello")
                    .create()
                    .await?;
            }

            let web = env.web_app().await;
            web.assert_redirect_cached_unchecked(
                &format!("/crate/{KRATE}/{req_version}/source/"),
                &format!("https://crates.io/crates/{KRATE}/{crates_io_version}/code/"),
                CachePolicy::ForeverInCdn(KRATE.into()),
                env.config(),
            )
            .await?;
            Ok(())
        })
    }

    #[test_case("^1", "1.0.0")]
    fn semver_handled(req_version: &'static str, crates_io_version: &'static str) {
        async_wrapper(|env| async move {
            for v in [V1, V2] {
                env.fake_release()
                    .await
                    .archive_storage(true)
                    .name(KRATE)
                    .version(v)
                    .source_file("README.md", b"hello")
                    .create()
                    .await?;
            }

            let web = env.web_app().await;
            web.assert_redirect_cached_unchecked(
                &format!("/crate/{KRATE}/{req_version}/source/"),
                &format!("https://crates.io/crates/{KRATE}/{crates_io_version}/code/"),
                CachePolicy::ForeverInCdnAndStaleInBrowser(KRATE.into()),
                env.config(),
            )
            .await?;
            Ok(())
        })
    }
}
