use crate::error::Result;
use crate::web::rustdoc::RustdocPage;
use anyhow::Context;
use rinja::Template;
use std::{fmt, sync::Arc};
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
                    // at which point we don't need the result any more.
                    let _ = send.send(render_fn());
                }
            }
        });

        recv.await.context("sender was dropped")?
    }
}

pub mod filters {
    use super::IconType;
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

    pub fn fas(value: &str, fw: bool, spin: bool, extra: &str) -> rinja::Result<Safe<String>> {
        IconType::Strong.render(value, fw, spin, extra).map(Safe)
    }

    pub fn far(value: &str, fw: bool, spin: bool, extra: &str) -> rinja::Result<Safe<String>> {
        IconType::Regular.render(value, fw, spin, extra).map(Safe)
    }

    pub fn fab(value: &str, fw: bool, spin: bool, extra: &str) -> rinja::Result<Safe<String>> {
        IconType::Brand.render(value, fw, spin, extra).map(Safe)
    }

    pub fn highlight(code: impl std::fmt::Display, lang: &str) -> rinja::Result<Safe<String>> {
        let highlighted_code = crate::web::highlight::with_lang(Some(lang), &code.to_string());
        Ok(Safe(format!(
            "<pre><code>{}</code></pre>",
            highlighted_code
        )))
    }

    pub fn slugify<T: AsRef<str>>(code: T) -> rinja::Result<String> {
        Ok(slug::slugify(code.as_ref()))
    }

    pub fn round(value: &f32, precision: u32) -> rinja::Result<String> {
        let multiplier = if precision == 0 {
            1.0
        } else {
            10.0_f32.powi(precision as _)
        };
        Ok(((multiplier * *value).round() / multiplier).to_string())
    }

    pub fn date(value: &DateTime<Utc>, format: &str) -> rinja::Result<String> {
        Ok(format!("{}", value.format(format)))
    }

    pub fn opt_date(value: &Option<DateTime<Utc>>, format: &str) -> rinja::Result<String> {
        if let Some(value) = value {
            date(value, format)
        } else {
            Ok(String::new())
        }
    }

    pub fn split_first<'a>(value: &'a str, pat: &str) -> rinja::Result<Option<&'a str>> {
        Ok(value.split(pat).next())
    }

    pub fn json_encode<T: ?Sized + serde::Serialize>(value: &T) -> rinja::Result<String> {
        Ok(serde_json::to_string(value).expect("`encode_json` failed"))
    }

    pub fn rest_menu_url(current_target: &str, inner_path: &str) -> rinja::Result<String> {
        if current_target.is_empty() {
            return Ok(String::new());
        }
        Ok(format!("/{current_target}/{inner_path}"))
    }
}

enum IconType {
    Strong,
    Regular,
    Brand,
}

impl fmt::Display for IconType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let icon = match self {
            Self::Strong => "solid",
            Self::Regular => "regular",
            Self::Brand => "brands",
        };

        f.write_str(icon)
    }
}

impl IconType {
    fn render(self, icon_name: &str, fw: bool, spin: bool, extra: &str) -> rinja::Result<String> {
        let type_ = match self {
            IconType::Strong => font_awesome_as_a_crate::Type::Solid,
            IconType::Regular => font_awesome_as_a_crate::Type::Regular,
            IconType::Brand => font_awesome_as_a_crate::Type::Brands,
        };

        let icon_file_string = font_awesome_as_a_crate::svg(type_, icon_name).unwrap_or("");

        let mut classes = vec!["fa-svg"];
        if fw {
            classes.push("fa-svg-fw");
        }
        if spin {
            classes.push("fa-svg-spin");
        }
        if !extra.is_empty() {
            classes.push(extra);
        }
        let icon = format!(
            "\
<span class=\"{class}\" aria-hidden=\"true\">{icon_file_string}</span>",
            class = classes.join(" "),
        );

        Ok(icon)
    }
}
