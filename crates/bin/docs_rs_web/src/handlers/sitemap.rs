use crate::{
    error::{AxumNope, AxumResult},
    extractors::{DbConnection, Path},
    impl_axum_webpage,
    page::templates::filters,
};
use askama::Template;
use async_stream::stream;
use axum::{
    body::{Body, Bytes},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::{TypedHeader, headers::ContentType};
use chrono::{TimeZone, Utc};
use docs_rs_mimes as mimes;
use futures_util::{StreamExt as _, pin_mut};
use tracing::{Span, error};
use tracing_futures::Instrument as _;

/// sitemap index
#[derive(Template)]
#[template(path = "core/sitemap/index.xml")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SitemapIndexXml {
    sitemaps: Vec<char>,
}

impl_axum_webpage! {
    SitemapIndexXml,
    content_type = "application/xml",
}

pub(crate) async fn sitemapindex_handler() -> impl IntoResponse {
    let sitemaps: Vec<char> = ('a'..='z').collect();

    SitemapIndexXml { sitemaps }
}

#[derive(Template)]
#[template(path = "core/sitemap/_item.xml")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SitemapItemXml {
    crate_name: String,
    last_modified: String,
    target_name: String,
}

const SITEMAP_HEADER: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n"#;

const SITEMAP_FOOTER: &[u8] = b"</urlset>\n";

pub(crate) async fn sitemap_handler(
    Path(letter): Path<String>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    if letter.len() != 1 {
        return Err(AxumNope::ResourceNotFound);
    } else if let Some(ch) = letter.chars().next()
        && !(ch.is_ascii_lowercase())
    {
        return Err(AxumNope::ResourceNotFound);
    }

    let stream_span = Span::current();

    let stream = stream!({
        let mut items: usize = 0;
        let mut streamed_bytes: usize = SITEMAP_HEADER.len();

        yield Ok(Bytes::from_static(SITEMAP_HEADER));

        let result = sqlx::query!(
            r#"SELECT crates.name,
                    releases.target_name,
                    MAX(releases.release_time) as "release_time!"
             FROM crates
             INNER JOIN releases ON releases.crate_id = crates.id
             WHERE
                rustdoc_status = true AND
                crates.name ILIKE $1
             GROUP BY crates.name, releases.target_name
             "#,
            format!("{letter}%"),
        )
        .fetch(&mut *conn);

        pin_mut!(result);
        while let Some(row) = result.next().await {
            let row = match row {
                Ok(row) => row,
                Err(err) => {
                    error!(?err, "error fetching row from database");
                    yield Err(AxumNope::InternalError(err.into()));
                    break;
                }
            };

            match (SitemapItemXml {
                crate_name: row.name,
                target_name: row
                    .target_name
                    .expect("when we have rustdoc_status=true, this field is filled"),
                last_modified: row
                    .release_time
                    // On Aug 27 2022 we added `<link rel="canonical">` to all pages,
                    // so they should all get recrawled if they haven't been since then.
                    .max(Utc.with_ymd_and_hms(2022, 8, 28, 0, 0, 0).unwrap())
                    .format("%+")
                    .to_string(),
            })
            .render()
            {
                Ok(item) => {
                    let bytes = Bytes::from(item);
                    items += 1;
                    streamed_bytes += bytes.len();
                    yield Ok(bytes);
                }
                Err(err) => {
                    error!(?err, "error when rendering sitemap item xml");
                    yield Err(AxumNope::InternalError(err.into()));
                    break;
                }
            };
        }

        streamed_bytes += SITEMAP_FOOTER.len();
        yield Ok(Bytes::from_static(SITEMAP_FOOTER));

        if items > 50_000 || streamed_bytes > 50 * 1024 * 1024 {
            // alert when sitemap limits are reached
            // https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap#general-guidelines
            error!(items, streamed_bytes, letter, "sitemap limits exceeded")
        }
    })
    .instrument(stream_span);

    Ok((
        StatusCode::OK,
        TypedHeader(ContentType::from(mimes::APPLICATION_XML.clone())),
        Body::from_stream(stream),
    ))
}

#[cfg(test)]
mod tests {
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironmentExt as _, async_wrapper,
    };
    use axum::http::StatusCode;

    #[test]
    fn sitemap_index() {
        async_wrapper(|env| async move {
            let app = env.web_app().await;
            app.assert_success("/sitemap.xml").await?;
            Ok(())
        })
    }

    #[test]
    fn sitemap_invalid_letters() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            // everything not length=1 and ascii-lowercase should fail
            for invalid_letter in &["1", "aa", "A", ""] {
                println!("trying to fail letter {invalid_letter}");
                assert_eq!(
                    web.get(&format!("/-/sitemap/{invalid_letter}/sitemap.xml"))
                        .await?
                        .status(),
                    StatusCode::NOT_FOUND
                );
            }
            Ok(())
        })
    }

    #[test]
    fn sitemap_letter() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            // letter-sitemaps always work, even without crates & releases
            for letter in 'a'..='z' {
                web.assert_success(&format!("/-/sitemap/{letter}/sitemap.xml"))
                    .await?;
            }

            env.fake_release()
                .await
                .name("some_random_crate")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("some_random_crate_that_failed")
                .build_result_failed()
                .create()
                .await?;

            // these fake crates appear only in the `s` sitemap
            let response = web.get("/-/sitemap/s/sitemap.xml").await?;
            assert!(response.status().is_success());

            let content = response.text().await?;
            assert!(content.contains("some_random_crate"));
            assert!(!(content.contains("some_random_crate_that_failed")));

            // and not in the others
            for letter in ('a'..='z').filter(|&c| c != 's') {
                let response = web.get(&format!("/-/sitemap/{letter}/sitemap.xml")).await?;

                assert!(response.status().is_success());
                assert!(!(response.text().await?.contains("some_random_crate")));
            }

            Ok(())
        })
    }

    #[test]
    fn sitemap_max_age() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            use chrono::{TimeZone, Utc};
            env.fake_release()
                .await
                .name("some_random_crate")
                .release_time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap())
                .create()
                .await?;

            let response = web.get("/-/sitemap/s/sitemap.xml").await?;
            assert!(response.status().is_success());

            let content = response.text().await?;
            assert!(content.contains("2022-08-28T00:00:00+00:00"));
            Ok(())
        })
    }
}
