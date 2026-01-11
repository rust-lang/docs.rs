use crate::handlers::rustdoc::RustdocPage;
use anyhow::{Context as _, Result};
use askama::Template;
use std::sync::Arc;
use tracing::trace;

#[derive(Template)]
#[template(path = "rustdoc/head.html")]
pub struct Head<'a> {
    rustdoc_css_file: Option<&'a str>,
}

impl<'a> Head<'a> {
    pub fn new(inner: &'a RustdocPage) -> Self {
        Self {
            rustdoc_css_file: inner.metadata.rustdoc_css_file.as_deref(),
        }
    }
}

#[derive(Template)]
#[template(path = "rustdoc/vendored.html")]
pub struct Vendored;

#[derive(Template)]
#[template(path = "rustdoc/body.html")]
pub struct Body;

/// Holds all data relevant to templating
#[derive(Debug)]
pub(crate) struct TemplateData {
    /// rendering threadpool for CPU intensive rendering.
    /// When the app is shut down, the pool won't wait
    /// for pending tasks in this pool.
    /// In the case of rendering, this is what we want.
    /// See also https://github.com/rayon-rs/rayon/issues/688.
    ///
    /// This is better than using `tokio::spawn_blocking` because
    /// tokio will wait until all tasks are finished when shutting
    /// down.
    rendering_threadpool: rayon_core::ThreadPool,
}

impl TemplateData {
    pub(crate) fn new(num_threads: usize) -> Result<Self> {
        trace!("Loading templates");

        let data = Self {
            rendering_threadpool: rayon_core::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .thread_name(move |idx| format!("docsrs-render {idx}"))
                .build()?,
        };

        trace!("Finished loading templates");

        Ok(data)
    }

    /// offload CPU intensive rendering into a rayon threadpool.
    ///
    /// This is a thin wrapper around `rayon::spawn` which waits
    /// sync task to finish.
    ///
    /// Use this instead of `spawn_blocking` so we don't block tokio.
    pub(crate) async fn render_in_threadpool<F, R>(self: &Arc<Self>, render_fn: F) -> Result<R>
    where
        F: FnOnce() -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let span = tracing::Span::current();
        let (send, recv) = tokio::sync::oneshot::channel();
        self.rendering_threadpool.spawn({
            move || {
                let _guard = span.enter();
                // the job may have been queued on the thread-pool for a while,
                // if the request was closed in the meantime the receiver should have
                // dropped and we don't need to bother rendering the template
                if !send.is_closed() {
                    // `.send` only fails when the receiver is dropped while we were rendering,
                    // at which point we don't need the result anymore.
                    let _ = send.send(render_fn());
                }
            }
        });

        recv.await.context("sender was dropped")?
    }
}

pub mod filters {
    use askama::Values;
    use askama::filters::Safe;
    use chrono::{DateTime, Utc};
    use std::borrow::Cow;
    use std::fmt::Display;
    use std::time::Duration;

    pub fn escape_html_inner(input: &str) -> askama::Result<impl Display> {
        if !input.chars().any(|c| "&<>\"'/".contains(c)) {
            return Ok(Cow::Borrowed(input));
        }
        let mut output = String::with_capacity(input.len() * 2);
        for c in input.chars() {
            match c {
                '&' => output.push_str("&amp;"),
                '<' => output.push_str("&lt;"),
                '>' => output.push_str("&gt;"),
                '"' => output.push_str("&quot;"),
                '\'' => output.push_str("&#x27;"),
                '/' => output.push_str("&#x2F;"),
                _ => output.push(c),
            }
        }

        // Not using shrink_to_fit() on purpose
        Ok(Cow::Owned(output))
    }

    // Copied from `tera`.
    #[askama::filter_fn]
    pub fn escape_xml(input: &str, _: &dyn Values) -> askama::Result<impl Display> {
        if !input.chars().any(|c| "&<>\"'".contains(c)) {
            return Ok(Cow::Borrowed(input));
        }
        let mut output = String::with_capacity(input.len() * 2);
        for c in input.chars() {
            match c {
                '&' => output.push_str("&amp;"),
                '<' => output.push_str("&lt;"),
                '>' => output.push_str("&gt;"),
                '"' => output.push_str("&quot;"),
                '\'' => output.push_str("&apos;"),
                _ => output.push(c),
            }
        }
        Ok(Cow::Owned(output))
    }

    /// Prettily format a timestamp
    // TODO: This can be replaced by chrono
    #[askama::filter_fn]
    pub fn timeformat(value: &DateTime<Utc>, _: &dyn Values) -> askama::Result<String> {
        Ok(crate::utils::duration_to_str(*value))
    }

    #[askama::filter_fn]
    pub fn format_duration(duration: &Duration, _: &dyn Values) -> askama::Result<Safe<String>> {
        let mut secs = duration.as_secs();

        let hours = secs / 3_600;
        secs %= 3_600;
        let minutes = secs / 60;
        let seconds = secs % 60;

        let mut parts = Vec::new();
        if hours > 0 {
            parts.push(format!("{hours}h"));
        }
        if minutes > 0 {
            parts.push(format!("{minutes}m"));
        }
        if seconds > 0 || parts.is_empty() {
            parts.push(format!("{seconds}s"));
        }

        Ok(Safe(parts.join(" ")))
    }

    /// Dedent a string by removing all leading whitespace
    #[allow(clippy::unnecessary_wraps)]
    #[askama::filter_fn]
    pub fn dedent<T: std::fmt::Display, I: Into<Option<i32>>>(
        value: T,
        _: &dyn Values,
        levels: I,
    ) -> askama::Result<String> {
        let string = value.to_string();

        let unindented = if let Some(levels) = levels.into() {
            string
                .lines()
                .map(|mut line| {
                    for _ in 0..levels {
                        // `.strip_prefix` returns `Some(suffix without prefix)` if it's successful. If it fails
                        // to strip the prefix (meaning there's less than `levels` levels of indentation),
                        // we can just quit early
                        if let Some(suffix) = line.strip_prefix("    ") {
                            line = suffix;
                        } else {
                            break;
                        }
                    }

                    line
                })
                .collect::<Vec<&str>>()
                .join("\n")
        } else {
            string
                .lines()
                .map(|l| l.trim_start())
                .collect::<Vec<&str>>()
                .join("\n")
        };

        Ok(unindented)
    }

    #[askama::filter_fn]
    pub fn highlight(
        code: impl std::fmt::Display,
        _: &dyn Values,
        lang: &str,
    ) -> askama::Result<Safe<String>> {
        let highlighted_code =
            crate::utils::highlight::with_lang(Some(lang), &code.to_string(), None);
        Ok(Safe(format!("<pre><code>{highlighted_code}</code></pre>")))
    }

    #[askama::filter_fn]
    pub fn round(value: &f32, _: &dyn Values, precision: u32) -> askama::Result<String> {
        let multiplier = if precision == 0 {
            1.0
        } else {
            10.0_f32.powi(precision as _)
        };
        Ok(((multiplier * *value).round() / multiplier).to_string())
    }

    #[askama::filter_fn]
    pub fn json_encode<T: ?Sized + serde::Serialize>(
        value: &T,
        _: &dyn Values,
    ) -> askama::Result<Safe<String>> {
        Ok(Safe(
            serde_json::to_string(value).expect("`encode_json` failed"),
        ))
    }
}

pub trait RenderSolid {
    fn render_solid(&self, fw: bool, spin: bool, extra: &str) -> askama::filters::Safe<String>;
}

impl<T: font_awesome_as_a_crate::Solid> RenderSolid for T {
    fn render_solid(&self, fw: bool, spin: bool, extra: &str) -> askama::filters::Safe<String> {
        render("fa-solid", self.icon_name(), fw, spin, extra)
    }
}

pub trait RenderRegular {
    fn render_regular(&self, fw: bool, spin: bool, extra: &str) -> askama::filters::Safe<String>;
}

impl<T: font_awesome_as_a_crate::Regular> RenderRegular for T {
    fn render_regular(&self, fw: bool, spin: bool, extra: &str) -> askama::filters::Safe<String> {
        render("fa-regular", self.icon_name(), fw, spin, extra)
    }
}

pub trait RenderBrands {
    fn render_brands(&self, fw: bool, spin: bool, extra: &str) -> askama::filters::Safe<String>;
}

impl<T: font_awesome_as_a_crate::Brands> RenderBrands for T {
    fn render_brands(&self, fw: bool, spin: bool, extra: &str) -> askama::filters::Safe<String> {
        render("fa-brands", self.icon_name(), fw, spin, extra)
    }
}

fn render(
    icon_kind: &str,
    css_class: &str,
    fw: bool,
    spin: bool,
    extra: &str,
) -> askama::filters::Safe<String> {
    let mut classes = Vec::new();
    if fw {
        classes.push("fa-fw");
    }
    if spin {
        classes.push("fa-spin");
    }
    if !extra.is_empty() {
        classes.push(extra);
    }
    let icon = format!(
        "<span class=\"fa {icon_kind} fa-{css_class} {classes}\" aria-hidden=\"true\"></span>",
        classes = classes.join(" "),
    );

    askama::filters::Safe(icon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{any::Any, collections::HashMap, time::Duration};
    use test_case::test_case;

    #[test_case(Duration::from_secs(0) => "0s"; "zero")]
    #[test_case(Duration::from_secs(1) => "1s"; "simple")]
    #[test_case(Duration::from_micros(2123456) => "2s"; "cuts microseconds")]
    #[test_case(Duration::from_secs(3723) => "1h 2m 3s"; "hours minutes seconds")]
    #[test_case(Duration::from_secs(120) => "2m"; "just minutes")]
    #[test_case(Duration::from_secs(2123456) => "589h 50m 56s"; "big")]
    fn test_format_duration(duration: Duration) -> String {
        let values: HashMap<&str, Box<dyn Any>> = HashMap::new();

        filters::format_duration::default()
            .execute(&duration, &values)
            .unwrap()
            .to_string()
    }
}
