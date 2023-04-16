use crate::web::highlight;
use comrak::{
    adapters::SyntaxHighlighterAdapter, ComrakExtensionOptions, ComrakOptions, ComrakPlugins,
    ComrakRenderPlugins,
};
use std::collections::HashMap;

#[derive(Debug)]
struct CodeAdapter<F>(F);

impl<F: Fn(Option<&str>, &str) -> String> SyntaxHighlighterAdapter for CodeAdapter<F> {
    fn write_highlighted(
        &self,
        output: &mut dyn std::io::Write,
        lang: Option<&str>,
        code: &str,
    ) -> std::io::Result<()> {
        // comrak does not treat `,` as an info-string delimiter, so we do that here
        // TODO: https://github.com/kivikakk/comrak/issues/246
        let lang = lang.and_then(|lang| lang.split(',').next());
        write!(output, "{}", (self.0)(lang, code))
    }

    fn write_pre_tag(
        &self,
        output: &mut dyn std::io::Write,
        attributes: HashMap<String, String>,
    ) -> std::io::Result<()> {
        write_opening_tag(output, "pre", &attributes)
    }

    fn write_code_tag(
        &self,
        output: &mut dyn std::io::Write,
        attributes: HashMap<String, String>,
    ) -> std::io::Result<()> {
        // similarly to above, since comrak does not treat `,` as an info-string delimiter it will
        // try to apply `class="language-rust,ignore"` for the info-string `rust,ignore`, so we
        // have to detect that case and fixup the class here
        // TODO: https://github.com/kivikakk/comrak/issues/246
        let mut attributes = attributes;
        if let Some(classes) = attributes.get_mut("class") {
            *classes = classes
                .split(' ')
                .flat_map(|class| [class.split(',').next().unwrap_or(class), " "])
                .collect();
            // remove trailing ' '
            // TODO: https://github.com/rust-lang/rust/issues/79524 or itertools
            classes.pop();
        }
        write_opening_tag(output, "code", &attributes)
    }
}

fn write_opening_tag(
    output: &mut dyn std::io::Write,
    tag: &str,
    attributes: &HashMap<String, String>,
) -> std::io::Result<()> {
    write!(output, "<{tag}")?;
    for (attr, val) in attributes {
        write!(output, " {attr}=\"{val}\"")?;
    }
    write!(output, ">")
}

fn render_with_highlighter(
    text: &str,
    highlighter: impl Fn(Option<&str>, &str) -> String,
) -> String {
    comrak::markdown_to_html_with_plugins(
        text,
        &ComrakOptions {
            extension: ComrakExtensionOptions {
                superscript: true,
                table: true,
                autolink: true,
                tasklist: true,
                strikethrough: true,
                ..ComrakExtensionOptions::default()
            },
            ..ComrakOptions::default()
        },
        &ComrakPlugins {
            render: ComrakRenderPlugins {
                codefence_syntax_highlighter: Some(&CodeAdapter(highlighter)),
                ..Default::default()
            },
        },
    )
}

/// Wrapper around the Markdown parser and renderer to render markdown
pub fn render(text: &str) -> String {
    render_with_highlighter(text, highlight::with_lang)
}

#[cfg(test)]
mod test {
    use super::render_with_highlighter;
    use indoc::indoc;
    use std::cell::RefCell;

    #[test]
    fn ignore_info_string_attributes() {
        let highlighted = RefCell::new(vec![]);

        let output = render_with_highlighter(
            indoc! {"
                ```rust,ignore
                ignore::commas();
                ```

                ```rust ignore
                ignore::spaces();
                ```
            "},
            |lang, code| {
                highlighted
                    .borrow_mut()
                    .push((lang.map(str::to_owned), code.to_owned()));
                code.to_owned()
            },
        );

        assert!(output.matches(r#"<code class="language-rust">"#).count() == 2);
        assert_eq!(
            highlighted.borrow().as_slice(),
            [
                (Some("rust".into()), "ignore::commas();\n".into()),
                (Some("rust".into()), "ignore::spaces();\n".into())
            ]
        );
    }
}
