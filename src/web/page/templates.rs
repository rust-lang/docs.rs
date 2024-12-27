use crate::error::Result;
use crate::web::rustdoc::RustdocPage;
use anyhow::Context;
use rinja::Template;
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
    rendering_threadpool: rayon::ThreadPool,
}

impl TemplateData {
    pub(crate) fn new(num_threads: usize) -> Result<Self> {
        trace!("Loading templates");

        let data = Self {
            rendering_threadpool: rayon::ThreadPoolBuilder::new()
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
        let (send, recv) = tokio::sync::oneshot::channel();
        self.rendering_threadpool.spawn({
            move || {
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
    use chrono::{DateTime, Utc};
    use rinja::filters::Safe;
    use std::borrow::Cow;

    // Copied from `tera`.
    pub fn escape_html(input: &str) -> rinja::Result<Cow<'_, str>> {
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
    pub fn escape_xml(input: &str) -> rinja::Result<Cow<'_, str>> {
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
    pub fn timeformat(value: &DateTime<Utc>) -> rinja::Result<String> {
        Ok(crate::web::duration_to_str(*value))
    }

    pub fn format_secs(mut value: f32) -> rinja::Result<String> {
        const TIMES: &[&str] = &["seconds", "minutes", "hours"];

        let mut chosen_time = &TIMES[0];

        for time in &TIMES[1..] {
            if value / 60.0 >= 1.0 {
                chosen_time = time;
                value /= 60.0;
            } else {
                break;
            }
        }

        // TODO: This formatting section can be optimized, two string allocations aren't needed
        let mut value = format!("{value:.1}");
        if value.ends_with(".0") {
            value.truncate(value.len() - 2);
        }

        Ok(format!("{value} {chosen_time}"))
    }

    /// Dedent a string by removing all leading whitespace
    #[allow(clippy::unnecessary_wraps)]
    pub fn dedent<T: std::fmt::Display, I: Into<Option<i32>>>(
        value: T,
        levels: I,
    ) -> rinja::Result<String> {
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

    pub fn highlight(code: impl std::fmt::Display, lang: &str) -> rinja::Result<Safe<String>> {
        let highlighted_code = crate::web::highlight::with_lang(Some(lang), &code.to_string());
        Ok(Safe(format!(
            "<pre><code>{}</code></pre>",
            highlighted_code
        )))
    }

    pub fn round(value: &f32, precision: u32) -> rinja::Result<String> {
        let multiplier = if precision == 0 {
            1.0
        } else {
            10.0_f32.powi(precision as _)
        };
        Ok(((multiplier * *value).round() / multiplier).to_string())
    }

    pub fn split_first<'a>(value: &'a str, pat: &str) -> rinja::Result<Option<&'a str>> {
        Ok(value.split(pat).next())
    }

    pub fn json_encode<T: ?Sized + serde::Serialize>(value: &T) -> rinja::Result<Safe<String>> {
        Ok(Safe(
            serde_json::to_string(value).expect("`encode_json` failed"),
        ))
    }
}

pub trait RenderSolid {
    fn render_solid(&self, fw: bool, spin: bool, extra: &str) -> rinja::filters::Safe<String>;
}

impl<T: font_awesome_as_a_crate::Solid> RenderSolid for T {
    fn render_solid(&self, fw: bool, spin: bool, extra: &str) -> rinja::filters::Safe<String> {
        render("fa-solid", self.icon_name(), fw, spin, extra)
    }
}

pub trait RenderRegular {
    fn render_regular(&self, fw: bool, spin: bool, extra: &str) -> rinja::filters::Safe<String>;
}

impl<T: font_awesome_as_a_crate::Regular> RenderRegular for T {
    fn render_regular(&self, fw: bool, spin: bool, extra: &str) -> rinja::filters::Safe<String> {
        render("fa-regular", self.icon_name(), fw, spin, extra)
    }
}

pub trait RenderBrands {
    fn render_brands(&self, fw: bool, spin: bool, extra: &str) -> rinja::filters::Safe<String>;
}

impl<T: font_awesome_as_a_crate::Brands> RenderBrands for T {
    fn render_brands(&self, fw: bool, spin: bool, extra: &str) -> rinja::filters::Safe<String> {
        render("fa-brands", self.icon_name(), fw, spin, extra)
    }
}

fn render(
    icon_kind: &str,
    css_class: &str,
    fw: bool,
    spin: bool,
    extra: &str,
) -> rinja::filters::Safe<String> {
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

    rinja::filters::Safe(icon)
}
