use crate::error::Result;
use crate::web::rustdoc::RustdocPage;
use anyhow::Context;
use askama::Template;
use std::{fmt, ops::Deref, sync::Arc};
use tracing::trace;

macro_rules! rustdoc_page {
    ($name:ident, $path:literal) => {
        #[derive(Template)]
        #[template(path = $path)]
        pub struct $name<'a> {
            inner: &'a RustdocPage,
        }

        impl<'a> $name<'a> {
            pub fn new(inner: &'a RustdocPage) -> Self {
                Self { inner }
            }
        }

        impl<'a> Deref for $name<'a> {
            type Target = RustdocPage;

            fn deref(&self) -> &Self::Target {
                self.inner
            }
        }
    };
}

rustdoc_page!(Head, "rustdoc/head.html");
rustdoc_page!(Vendored, "rustdoc/vendored.html");
rustdoc_page!(Body, "rustdoc/body.html");
rustdoc_page!(Topbar, "rustdoc/topbar.html");

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
    use std::borrow::Cow;
    use std::str::FromStr;

    // Copied from `tera`.
    #[inline]
    pub fn escape_html(input: &str) -> askama::Result<Cow<'_, str>> {
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
    #[inline]
    pub fn escape_xml(input: &str) -> askama::Result<Cow<'_, str>> {
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
    #[allow(clippy::unnecessary_wraps)]
    pub fn timeformat(value: &str, is_relative: Option<bool>) -> askama::Result<String> {
        let fmt = if let Some(true) = is_relative {
            let value = DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&Utc);

            crate::web::duration_to_str(value)
        } else {
            const TIMES: &[&str] = &["seconds", "minutes", "hours"];

            let mut value = f64::from_str(value).unwrap();
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

            format!("{value} {chosen_time}")
        };

        Ok(fmt)
    }

    /// Print a value to stdout
    #[allow(clippy::unnecessary_wraps)]
    pub fn dbg<T: std::fmt::Display>(value: T) -> askama::Result<String> {
        let value = value.to_string();
        println!("{value}");

        Ok(value)
    }

    /// Dedent a string by removing all leading whitespace
    #[allow(clippy::unnecessary_wraps)]
    pub fn dedent<T: std::fmt::Display>(value: String, levels: Option<i64>) -> askama::Result<String> {
        let string = value.to_string();

        let unindented = if let Some(levels) = levels {
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

    pub fn fas(value: &str, fw: bool, extra: &str) -> askama::Result<String> {
        IconType::Strong.render(value, fw, extra)
    }

    pub fn far(value: &str, fw: bool, extra: &str) -> askama::Result<String> {
        IconType::Regular.render(value, fw, extra)
    }

    pub fn fab(value: &str, fw: bool, extra: &str) -> askama::Result<String> {
        IconType::Brand.render(value, fw, extra)
    }

    pub fn highlight(code: &str, lang: &str) -> askama::Result<String> {
        let highlighted_code = crate::web::highlight::with_lang(Some(lang), code);
        Ok(format!("<pre><code>{}</code></pre>", highlighted_code))
    }

    pub fn slugify<T: AsRef<str>>(code: T) -> askama::Result<String> {
        Ok(slug::slugify(code.as_ref()))
    }

    pub fn round(value: f32, precision: u32) -> askama::Result<String> {
        use std::fmt;

        struct Rounder(f32, u32);

        impl fmt::Display for Rounder {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{1:.*})", self.1, self.0)
            }
        }
        Ok(format!("{}", Rounder(value, precision)))
    }

    pub fn date(value: DateTime<Utc>, format: &str) -> askama::Result<String> {
        Ok(format!("{}", value.format(format)))
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
    fn render(self, icon_name: &str, fw: bool, extra: &str) -> askama::Result<String> {
        let class = if fw {
            "fa-svg fa-svg-fw"
        } else {
            "fa-svg"
        };

        let type_ = match self {
            IconType::Strong => font_awesome_as_a_crate::Type::Solid,
            IconType::Regular => font_awesome_as_a_crate::Type::Regular,
            IconType::Brand => font_awesome_as_a_crate::Type::Brands,
        };

        let icon_file_string = font_awesome_as_a_crate::svg(type_, &icon_name[..]).unwrap_or("");
        let (space, class_extra) = if !extra.is_empty() {
            (" ", extra)
        } else {
            ("", "")
        };

        let icon = format!("<span class=\"{class}{space}{class_extra}\">{icon_file_string}</span>");

        Ok(icon)
    }
}
