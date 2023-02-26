use crate::web::highlight;
use comrak::{
    adapters::SyntaxHighlighterAdapter, ComrakExtensionOptions, ComrakOptions, ComrakPlugins,
    ComrakRenderPlugins,
};
use std::{collections::HashMap, fmt::Write};

#[derive(Debug)]
struct CodeAdapter<F>(F);

impl<F: Fn(Option<&str>, &str) -> String> SyntaxHighlighterAdapter for CodeAdapter<F> {
    fn highlight(&self, lang: Option<&str>, code: &str) -> String {
        // comrak does not treat `,` as an info-string delimiter, so we do that here
        // TODO: https://github.com/kivikakk/comrak/issues/246
        let lang = lang.and_then(|lang| lang.split(',').next());
        (self.0)(lang, code)
    }

    fn build_pre_tag(&self, attributes: &HashMap<String, String>) -> String {
        build_opening_tag("pre", attributes)
    }

    fn build_code_tag(&self, attributes: &HashMap<String, String>) -> String {
        // similarly to above, since comrak does not treat `,` as an info-string delimiter it will
        // try to apply `class="language-rust,ignore"` for the info-string `rust,ignore`, so we
        // have to detect that case and fixup the class here
        // TODO: https://github.com/kivikakk/comrak/issues/246
        let mut attributes = attributes.clone();
        if let Some(classes) = attributes.get_mut("class") {
            *classes = classes
                .split(' ')
                .flat_map(|class| [class.split(',').next().unwrap_or(class), " "])
                .collect();
            // remove trailing ' '
            // TODO: https://github.com/rust-lang/rust/issues/79524 or itertools
            classes.pop();
        }
        build_opening_tag("code", &attributes)
    }
}

fn build_opening_tag(tag: &str, attributes: &HashMap<String, String>) -> String {
    let mut tag_parts = format!("<{tag}");
    for (attr, val) in attributes {
        write!(tag_parts, " {attr}=\"{val}\"").unwrap();
    }
    tag_parts.push('>');
    tag_parts
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
