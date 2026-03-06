use crate::{
    handlers::rustdoc::RustdocPage,
    metrics::WebMetrics,
    page::{
        TemplateData,
        templates::{Body, Head, Vendored},
    },
};
use anyhow::{Context as _, anyhow};
use askama::Template;
use async_stream::stream;
use axum::body::Bytes;
use futures_util::{Stream, StreamExt as _};
use lol_html::{element, errors::RewritingError};
use std::sync::Arc;
use tokio::{io::AsyncRead, task::JoinHandle};
use tokio_util::io::ReaderStream;
use tracing::{Span, error, instrument};
use tracing_futures::Instrument as _;

const CHANNEL_SIZE: usize = 64;

#[derive(thiserror::Error, Debug)]
pub(crate) enum RustdocRewritingError {
    #[error("HTML rewriter error: {0}")]
    RewritingError(#[from] lol_html::errors::RewritingError),
    #[error("generic error while rewriting rustdoc HTML: {0}")]
    Other(#[from] anyhow::Error),
}

/// Rewrite a rustdoc page to have the docs.rs topbar
///
/// Given a rustdoc HTML page and a context to serialize it with,
/// render the `rustdoc/` templates with the `html`.
/// The output is an HTML page which has not yet been UTF-8 validated.
/// In practice, the output should always be valid UTF-8.
#[instrument(skip_all, fields(memory_limit = max_allowed_memory_usage))]
pub(crate) fn rewrite_rustdoc_html_stream<R>(
    template_data: Arc<TemplateData>,
    mut reader: R,
    max_allowed_memory_usage: usize,
    data: Arc<RustdocPage>,
    otel_metrics: Arc<WebMetrics>,
) -> impl Stream<Item = Result<Bytes, RustdocRewritingError>> + Send + 'static
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let stream_span = Span::current();

    stream!({
        let (input_sender, mut input_receiver) =
            tokio::sync::mpsc::channel::<Option<Bytes>>(CHANNEL_SIZE);
        let (result_sender, mut result_receiver) =
            tokio::sync::mpsc::channel::<Bytes>(CHANNEL_SIZE);

        let producer_span = tracing::info_span!("producer_task");

        let join_handle: JoinHandle<anyhow::Result<_>> = tokio::spawn(
            async move {
                // we're using the rendering threadpool to limit CPU usage on the server, and to
                // offload potentially CPU intensive stuff from the tokio runtime.
                // Also this lets us limit the threadpool size and through that the CPU usage.
                let render_span = tracing::info_span!("render_task");
                template_data
                    .render_in_threadpool(move || {
                        use lol_html::html_content::{ContentType, Element};
                        use lol_html::{HtmlRewriter, MemorySettings, Settings};

                        let head_html = Head::new(&data).render().unwrap();
                        let vendored_html = Vendored.render().unwrap();
                        let body_html = Body.render().unwrap();
                        let topbar_html = data.render().unwrap();

                        // Before: <body> ... rustdoc content ... </body>
                        // After:
                        // ```html
                        // <div id="rustdoc_body_wrapper" class="{{ rustdoc_body_class }}" tabindex="-1">
                        //      ... rustdoc content ...
                        // </div>
                        // ```
                        let body_handler = |rustdoc_body_class: &mut Element| {
                            // Add the `rustdoc` classes to the html body
                            let mut tmp;
                            let klass =
                                if let Some(classes) = rustdoc_body_class.get_attribute("class") {
                                    tmp = classes;
                                    tmp.push_str(" container-rustdoc");
                                    &tmp
                                } else {
                                    "container-rustdoc"
                                };
                            rustdoc_body_class.set_attribute("class", klass)?;
                            rustdoc_body_class.set_attribute("id", "rustdoc_body_wrapper")?;
                            rustdoc_body_class.set_attribute("tabindex", "-1")?;
                            // Change the `body` to a `div`
                            rustdoc_body_class.set_tag_name("div")?;
                            // Prepend the askama content
                            rustdoc_body_class.prepend(&body_html, ContentType::Html);
                            // Wrap the transformed body and topbar into a <body> element
                            rustdoc_body_class
                                .before(r#"<body class="rustdoc-page">"#, ContentType::Html);
                            // Insert the topbar outside of the rustdoc div
                            rustdoc_body_class.before(&topbar_html, ContentType::Html);
                            // Finalize body with </body>
                            rustdoc_body_class.after("</body>", ContentType::Html);

                            Ok(())
                        };

                        let settings = Settings {
                            element_content_handlers: vec![
                                // Append `style.css` stylesheet after all head elements.
                                element!("head", |head: &mut Element| {
                                    head.append(&head_html, ContentType::Html);
                                    Ok(())
                                }),
                                element!("body", body_handler),
                                // Append `vendored.css` before `rustdoc.css`, so that the duplicate copy of
                                // `normalize.css` will be overridden by the later version.
                                //
                                // Later rustdoc has `#mainThemeStyle` that could be used, but pre-2018 docs
                                // don't have this:
                                //
                                // https://github.com/rust-lang/rust/commit/003b2bc1c65251ec2fc80b78ed91c43fb35402ec
                                //
                                // Pre-2018 rustdoc also didn't have the resource suffix, but docs.rs was using a fork
                                // that had implemented it already then, so we can assume the css files are
                                // `<some path>/rustdoc-<some suffix>.css` and use the `-` to distinguish from the
                                // `rustdoc.static` path.
                                element!(
                                    "link[rel='stylesheet'][href*='rustdoc-']",
                                    move |rustdoc_css: &mut Element| {
                                        rustdoc_css.before(&vendored_html, ContentType::Html);
                                        Ok(())
                                    }
                                ),
                            ],
                            memory_settings: MemorySettings {
                                max_allowed_memory_usage,
                                ..MemorySettings::default()
                            },
                            ..Settings::default()
                        };

                        let mut rewriter = HtmlRewriter::new(settings, move |chunk: &[u8]| {
                            // send the result back to the main rewriter when its coming in.
                            // this can fail only when the receiver is dropped, in which case
                            // we exit this thread anyways.
                            let _ = result_sender.blocking_send(Bytes::copy_from_slice(chunk));
                        });
                        while let Some(chunk) = input_receiver
                            .blocking_recv()
                            .ok_or_else(|| anyhow!("couldn't receive from input_receiver"))?
                        {
                            // receive data from the input receiver.
                            // `input_receiver` is a non-async one.
                            // Since we're in a normal background thread, we can use the blocking `.recv`
                            // here.
                            // We will get `None` when the reader is done reading,
                            // so that's our signal to exit this loop and call `rewriter.end()` below.
                            rewriter.write(&chunk)?;
                        }
                        // finalize everything. Will trigger the output sink (and through that,
                        // sending data to the `result_sender`).
                        rewriter.end()?;
                        Ok(())
                    })
                    .instrument(render_span)
                    .await?;
                Ok(())
            }
            .instrument(producer_span),
        );

        let mut reader_stream = ReaderStream::new(&mut reader);
        while let Some(chunk) = reader_stream.next().await {
            let chunk = chunk.map_err(|err| {
                error!(?err, "error while reading from rustdoc HTML reader");
                RustdocRewritingError::Other(err.into())
            })?;

            if let Err(err) = input_sender.send(Some(chunk)).await {
                error!(
                    ?err,
                    "error when trying to send chunk to html rewriter thread"
                );
                yield Err(RustdocRewritingError::Other(err.into()));
                break;
            }

            while let Ok(bytes) = result_receiver.try_recv() {
                yield Ok(bytes);
            }
        }
        // This signals the renderer thread to finalize & exit.
        if let Err(err) = input_sender.send(None).await {
            error!(
                ?err,
                "error when trying to send end signal to html rewriter thread"
            );
            yield Err(RustdocRewritingError::Other(err.into()));
        }
        while let Some(bytes) = result_receiver.recv().await {
            yield Ok(bytes);
        }

        join_handle
            .await
            .context("task join failed")?
            .map_err(|e| {
                error!(?e, "error while rewriting rustdoc HTML");
                // our `render_in_threadpool` and so the async tokio task return an `anyhow::Result`.
                // In most cases this will be an error from the `HtmlRewriter`, which we'll get as a
                // `RewritingError` which we extract here again. The other cases remain an
                // `anyhow::Error`.
                match e.downcast::<RewritingError>() {
                    Ok(e) => {
                        if matches!(e, RewritingError::MemoryLimitExceeded(_)) {
                            otel_metrics.html_rewrite_ooms.add(1, &[]);
                        }
                        RustdocRewritingError::RewritingError(e)
                    }
                    Err(e) => RustdocRewritingError::Other(e),
                }
            })?;
    })
    .instrument(stream_span)
}

#[cfg(test)]
mod test {
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironmentExt as _, async_wrapper,
    };
    use docs_rs_types::testing::V1;

    #[test]
    fn rewriting_only_injects_css_once() {
        async_wrapper(|env| async move {
            env.fake_release().await
                .name("testing")
                .version(V1)
                // A somewhat representative rustdoc html file from 2016
                .rustdoc_file_with("2016/index.html", br#"
                    <html>
                        <head>
                            <meta charset="utf-8">
                            <link rel="stylesheet" type="text/css" href="../../../rustdoc-20160728-1.12.0-nightly-54c0dcfd6.css">
                            <link rel="stylesheet" type="text/css" href="../../../main-20160728-1.12.0-nightly-54c0dcfd6.css">
                        </head>
                    </html>
                "#)
                // A somewhat representative rustdoc html file from late 2022
                .rustdoc_file_with("2022/index.html", br#"
                    <html>
                        <head>
                            <meta charset="utf-8">
                            <link rel="preload" as="font" type="font/woff2" crossorigin="" href="/-/rustdoc.static/SourceSerif4-Regular-1f7d512b176f0f72.ttf.woff2">
                            <link rel="stylesheet" href="/-/rustdoc.static/normalize-76eba96aa4d2e634.css">
                            <link rel="stylesheet" href="/-/rustdoc.static/rustdoc-eabf764633b9d7be.css" id="mainThemeStyle">
                            <link rel="stylesheet" disabled="" href="/-/rustdoc.static/dark-e2f4109f2e82e3af.css">
                            <script src="/-/rustdoc.static/storage-d43fa987303ecbbb.js"></script>
                            <noscript><link rel="stylesheet" href="/-/rustdoc.static/noscript-13285aec31fa243e.css"></noscript>
                        </head>
                    </html>
                "#)
                .create().await?;

            let web = env.web_app().await;
            let output = web
                .get(&format!("/testing/{V1}/2016/"))
                .await?
                .text()
                .await?;
            assert_eq!(output.matches(r#"href="/-/static/vendored.css"#).count(), 1);

            let output = web
                .get(&format!("/testing/{V1}/2022/"))
                .await?
                .text()
                .await?;
            assert_eq!(output.matches(r#"href="/-/static/vendored.css"#).count(), 1);

            Ok(())
        });
    }
}
