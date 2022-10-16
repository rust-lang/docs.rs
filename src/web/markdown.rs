use crate::error::Result;
use comrak::{
    adapters::SyntaxHighlighterAdapter, ComrakExtensionOptions, ComrakOptions, ComrakPlugins,
    ComrakRenderPlugins,
};
use once_cell::sync::Lazy;
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

pub fn try_highlight_code(lang: Option<&str>, code: &str) -> Result<String> {
    use syntect::{
        html::{ClassStyle, ClassedHTMLGenerator},
        parsing::SyntaxSet,
        util::LinesWithEndings,
    };

    static SYNTAX_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/syntect.packdump"));
    static SYNTAXES: Lazy<SyntaxSet> = Lazy::new(|| {
        let syntaxes: SyntaxSet = syntect::dumps::from_uncompressed_data(SYNTAX_DATA).unwrap();
        let names = syntaxes
            .syntaxes()
            .iter()
            .map(|s| &s.name)
            .collect::<Vec<_>>();
        log::debug!("known syntaxes {names:?}");
        syntaxes
    });

    let syntax = lang
        .and_then(|lang| SYNTAXES.find_syntax_by_token(lang))
        .or_else(|| SYNTAXES.find_syntax_by_first_line(code))
        .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());

    log::trace!("Using syntax {:?} for language {lang:?}", syntax.name);

    let mut html_generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        &SYNTAXES,
        ClassStyle::SpacedPrefixed { prefix: "syntax-" },
    );

    for line in LinesWithEndings::from(code) {
        html_generator.parse_html_for_line_which_includes_newline(line)?;
    }

    Ok(html_generator.finalize())
}

pub fn highlight_code(lang: Option<&str>, code: &str) -> String {
    match try_highlight_code(lang, code) {
        Ok(highlighted) => highlighted,
        Err(err) => {
            log::error!("failed while highlighting code: {err:?}");
            code.to_owned()
        }
    }
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
            },
        },
    )
}

/// Wrapper around the Markdown parser and renderer to render markdown
pub fn render(text: &str) -> String {
    render_with_highlighter(text, highlight_code)
}

#[cfg(test)]
mod test {
    use super::{highlight_code, render_with_highlighter};
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
                highlight_code(lang, code)
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
