use crate::utils::highlight;
use comrak::{
    Options,
    adapters::{SyntaxHighlightMeta, SyntaxHighlighterAdapter},
    options,
};
use std::{borrow::Cow, collections::BTreeMap, fmt};

type Attributes<'s, 't> = BTreeMap<&'s str, Cow<'t, str>>;

#[derive(Debug)]
struct CodeAdapter<F>(F, Option<&'static str>);

impl<F: Fn(Option<&str>, &str, Option<&str>) -> String + Send + Sync> SyntaxHighlighterAdapter
    for CodeAdapter<F>
{
    fn write_highlighted(
        &self,
        output: &mut dyn fmt::Write,
        highlight: &SyntaxHighlightMeta<'_>,
    ) -> Result<(), fmt::Error> {
        // comrak does not treat `,` as an info-string delimiter, so we do that here
        // TODO: https://github.com/kivikakk/comrak/issues/246
        let lang = highlight.lang.split(',').next();
        write!(output, "{}", (self.0)(lang, highlight.code, self.1))
    }

    fn write_pre_tag(
        &self,
        output: &mut dyn fmt::Write,
        _highlight: &SyntaxHighlightMeta<'_>,
        attributes: Attributes<'_, '_>,
    ) -> fmt::Result {
        write_opening_tag(output, "pre", &attributes)
    }

    fn write_code_tag(
        &self,
        output: &mut dyn fmt::Write,
        _highlight: &SyntaxHighlightMeta<'_>,
        attributes: Attributes<'_, '_>,
    ) -> fmt::Result {
        // similarly to above, since comrak does not treat `,` as an info-string delimiter it will
        // try to apply `class="language-rust,ignore"` for the info-string `rust,ignore`, so we
        // have to detect that case and fixup the class here
        // TODO: https://github.com/kivikakk/comrak/issues/246
        let mut attributes = attributes;
        if let Some(classes) = attributes.get_mut("class") {
            let mut updated: String = classes
                .split(' ')
                .flat_map(|class| [class.split(',').next().unwrap_or(class), " "])
                .collect();
            // remove trailing ' '
            // TODO: https://github.com/rust-lang/rust/issues/79524 or itertools
            updated.pop();
            *classes = Cow::Owned(updated);
        }
        write_opening_tag(output, "code", &attributes)
    }
}

fn write_opening_tag<'s, 't>(
    output: &mut dyn fmt::Write,
    tag: &str,
    attributes: &Attributes<'s, 't>,
) -> Result<(), fmt::Error> {
    write!(output, "<{tag}")?;
    for (attr, val) in attributes {
        write!(output, " {attr}=\"{val}\"")?;
    }
    write!(output, ">")
}

fn render_with_highlighter(
    text: &str,
    default_syntax: Option<&'static str>,
    highlighter: impl Fn(Option<&str>, &str, Option<&str>) -> String + Send + Sync,
) -> String {
    let code_adapter = CodeAdapter(highlighter, default_syntax);

    let mut options = Options::default();
    options.extension.superscript = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.strikethrough = true;

    let mut plugins = options::Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&code_adapter);

    comrak::markdown_to_html_with_plugins(text, &options, &plugins)
}

/// Wrapper around the Markdown parser and renderer to render markdown
pub fn render(text: &str) -> String {
    render_with_highlighter(text, None, highlight::with_lang)
}

pub fn render_with_default(text: &str, default: &'static str) -> String {
    render_with_highlighter(text, Some(default), highlight::with_lang)
}

#[cfg(test)]
mod test {
    use super::render_with_highlighter;
    use indoc::indoc;
    use std::sync::Mutex;

    #[test]
    fn ignore_info_string_attributes() {
        let highlighted = Mutex::new(vec![]);

        let output = render_with_highlighter(
            indoc! {"
                ```rust,ignore
                ignore::commas();
                ```

                ```rust ignore
                ignore::spaces();
                ```
            "},
            None,
            |lang, code, _| {
                let mut highlighted = highlighted.lock().unwrap();
                highlighted.push((lang.map(str::to_owned), code.to_owned()));
                code.to_owned()
            },
        );

        assert!(output.matches(r#"<code class="language-rust">"#).count() == 2);
        let highlighted = highlighted.lock().unwrap();
        assert_eq!(
            highlighted.as_slice(),
            [
                (Some("rust".into()), "ignore::commas();\n".into()),
                (Some("rust".into()), "ignore::spaces();\n".into())
            ]
        );
    }
}
