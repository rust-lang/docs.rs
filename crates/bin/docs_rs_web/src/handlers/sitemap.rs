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
use chrono::{Days, NaiveDate, TimeZone, Utc};
use docs_rs_mimes as mimes;
use futures_util::{StreamExt as _, pin_mut, stream::BoxStream};
use tracing::{Span, error};
use tracing_futures::Instrument as _;

const RECENT_SITEMAP_DAYS: u64 = 7;

/// sitemap index
#[derive(Template)]
#[template(path = "core/sitemap/index.xml")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SitemapIndexXml {
    sitemaps: Vec<char>,
    recent_sitemaps: Vec<String>,
}

impl_axum_webpage! {
    SitemapIndexXml,
    content_type = "application/xml",
}

pub(crate) async fn sitemapindex_handler() -> impl IntoResponse {
    let sitemaps: Vec<char> = ('a'..='z').collect();
    let today = Utc::now().date_naive();
    let recent_sitemaps = (0..RECENT_SITEMAP_DAYS)
        .map(|days| {
            today
                .checked_sub_days(Days::new(days))
                .expect("underflow when building recent sitemap dates")
                .format("%F")
                .to_string()
        })
        .collect();

    SitemapIndexXml {
        sitemaps,
        recent_sitemaps,
    }
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

fn render_sitemap_item(item: SitemapItem) -> AxumResult<Bytes> {
    let target_name = item
        .target_name
        .expect("when we have rustdoc_status=true, this field is filled");

    let item = (SitemapItemXml {
        crate_name: item.name,
        target_name,
        last_modified: item
            .last_build_time
            .expect("when we have rustdoc_status=true, this field is filled")
            // On Aug 27 2022 we added `<link rel="canonical">` to all pages,
            // so they should all get recrawled if they haven't been since then.
            .max(Utc.with_ymd_and_hms(2022, 8, 28, 0, 0, 0).unwrap())
            .format("%+")
            .to_string(),
    })
    .render()
    .map_err(|err| AxumNope::InternalError(err.into()))?;

    Ok(Bytes::from(item))
}

struct SitemapItem {
    name: String,
    target_name: Option<String>,
    last_build_time: Option<chrono::DateTime<Utc>>,
}

type SitemapQueryStream<'a> = BoxStream<'a, Result<SitemapItem, sqlx::Error>>;

fn stream_sitemap<Query>(mut conn: DbConnection, query: Query) -> impl IntoResponse
where
    Query: for<'a> FnOnce(&'a mut DbConnection) -> SitemapQueryStream<'a> + Send + 'static,
{
    let stream_span = Span::current();
    let stream = stream!({
        let mut items: usize = 0;
        let mut streamed_bytes: usize = SITEMAP_HEADER.len();

        yield Ok(Bytes::from_static(SITEMAP_HEADER));

        let result = query(&mut conn);
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

            match render_sitemap_item(row) {
                Ok(bytes) => {
                    items += 1;
                    streamed_bytes += bytes.len();
                    yield Ok(bytes);
                }
                Err(err) => {
                    error!(?err, "error when rendering sitemap item xml");
                    yield Err(err);
                    break;
                }
            }
        }

        streamed_bytes += SITEMAP_FOOTER.len();
        yield Ok(Bytes::from_static(SITEMAP_FOOTER));

        if items > 50_000 || streamed_bytes > 50 * 1024 * 1024 {
            // alert when sitemap limits are reached
            // https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap#general-guidelines
            error!(items, streamed_bytes, "sitemap limits exceeded");
        }
    })
    .instrument(stream_span);

    (
        StatusCode::OK,
        TypedHeader(ContentType::from(mimes::APPLICATION_XML.clone())),
        Body::from_stream(stream),
    )
}

pub(crate) async fn sitemap_handler(
    Path(letter): Path<String>,
    conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    if letter.len() != 1 {
        return Err(AxumNope::ResourceNotFound);
    } else if let Some(ch) = letter.chars().next()
        && !ch.is_ascii_lowercase()
    {
        return Err(AxumNope::ResourceNotFound);
    }

    let letter_pattern = format!("{letter}%");
    Ok(stream_sitemap(conn, move |conn| {
        Box::pin(
            sqlx::query_as!(
                SitemapItem,
                r#"SELECT crates.name,
                            releases.target_name,
                            release_build_status.last_build_time
                     FROM crates
                     INNER JOIN releases ON crates.latest_version_id = releases.id
                     INNER JOIN release_build_status ON release_build_status.rid = releases.id
                     WHERE
                         rustdoc_status = true AND
                         crates.name ILIKE $1
                      "#,
                letter_pattern,
            )
            .fetch(&mut **conn),
        )
    }))
}

pub(crate) async fn recent_sitemap_handler(
    Path(date): Path<NaiveDate>,
    conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    let next_day = date
        .checked_add_days(Days::new(1))
        .ok_or(AxumNope::ResourceNotFound)?;

    let day_start = Utc.from_utc_datetime(
        &date
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 is always a valid time"),
    );
    let day_end = Utc.from_utc_datetime(
        &next_day
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 is always a valid time"),
    );

    Ok(stream_sitemap(conn, move |conn| {
        Box::pin(
            sqlx::query_as!(
                SitemapItem,
                r#"SELECT crates.name,
                            releases.target_name,
                            release_build_status.last_build_time
                     FROM crates
                     INNER JOIN releases ON crates.latest_version_id = releases.id
                     INNER JOIN release_build_status ON release_build_status.rid = releases.id
                     WHERE
                         releases.rustdoc_status = true AND
                         release_build_status.last_build_time >= $1 AND
                         release_build_status.last_build_time < $2
                     ORDER BY release_build_status.last_build_time DESC
                      "#,
                day_start,
                day_end,
            )
            .fetch(&mut **conn),
        )
    }))
}

#[cfg(test)]
mod tests {
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
    };
    use anyhow::Result;
    use axum::http::StatusCode;
    use chrono::{TimeZone as _, Utc};
    use test_case::test_case;

    #[tokio::test(flavor = "multi_thread")]
    async fn sitemap_index() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let app = env.web_app().await;
        let response = app.get("/sitemap.xml").await?;
        assert!(response.status().is_success());

        let content = response.text().await?;
        let today = Utc::now().date_naive();
        let expected_recent = format!("https://docs.rs/-/sitemap/recent/{today}/sitemap.xml",);
        assert!(content.contains(&expected_recent));
        Ok(())
    }

    #[test_case("1")]
    #[test_case("aa")]
    #[test_case("A")]
    #[test_case("")]
    #[tokio::test(flavor = "multi_thread")]
    async fn sitemap_invalid_letters(invalid_letter: &str) -> Result<()> {
        // everything not length=1 and ascii-lowercase should fail
        let env = TestEnvironment::new().await?;
        let web = env.web_app().await;

        assert_eq!(
            web.get(&format!("/-/sitemap/{invalid_letter}/sitemap.xml"))
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sitemap_letter() -> Result<()> {
        let env = TestEnvironment::new().await?;
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
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sitemap_max_age() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let web = env.web_app().await;
        let db = env.pool()?;

        env.fake_release()
            .await
            .name("some_random_crate")
            .release_time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap())
            .create()
            .await?;

        sqlx::query!(
            r#"UPDATE release_build_status
                   SET last_build_time = $1
                   FROM releases
                   INNER JOIN crates ON crates.id = releases.crate_id
                   WHERE release_build_status.rid = releases.id
                     AND crates.name = $2"#,
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            "some_random_crate",
        )
        .execute(&mut *db.get_async().await?)
        .await?;

        let response = web.get("/-/sitemap/s/sitemap.xml").await?;
        assert!(response.status().is_success());

        let content = response.text().await?;
        assert!(content.contains("2022-08-28T00:00:00+00:00"));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sitemap_recent_dates() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let web = env.web_app().await;

        let now = Utc::now();
        let today = now.date_naive().to_string();

        env.fake_release()
            .await
            .name("recent_sitemap_crate")
            .create()
            .await?;
        env.fake_release()
            .await
            .name("recent_sitemap_crate_failed")
            .build_result_failed()
            .create()
            .await?;

        {
            let response = web
                .assert_success(&format!("/-/sitemap/recent/{today}/sitemap.xml"))
                .await?;

            let content = response.text().await?;
            assert!(content.contains("recent_sitemap_crate"));
            assert!(!content.contains("recent_sitemap_crate_failed"));
        }

        {
            let response = web
                .assert_success("/-/sitemap/recent/1970-01-01/sitemap.xml")
                .await?;

            let content = response.text().await?;
            assert!(!content.contains("recent_sitemap_crate"));
            assert!(!content.contains("recent_sitemap_crate_failed"));
        }

        Ok(())
    }

    #[test_case("invalid-date")]
    #[test_case("2024-13-40")]
    #[tokio::test(flavor = "multi_thread")]
    async fn sitemap_recent_invalid_dates(invalid_date: &str) -> Result<()> {
        let env = TestEnvironment::new().await?;

        let web = env.web_app().await;

        assert_eq!(
            web.get(&format!("/-/sitemap/recent/{invalid_date}/sitemap.xml"))
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );

        Ok(())
    }
}
