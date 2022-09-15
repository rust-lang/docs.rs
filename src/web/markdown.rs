use crate::error::Result;
use comrak::{
    adapters::SyntaxHighlighterAdapter, ComrakExtensionOptions, ComrakOptions, ComrakPlugins,
    ComrakRenderPlugins,
};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fmt::Write;

#[derive(Debug)]
struct CodeAdapter;

impl SyntaxHighlighterAdapter for CodeAdapter {
    fn highlight(&self, lang: Option<&str>, code: &str) -> String {
        highlight_code(lang, code)
    }

    fn build_pre_tag(&self, attributes: &HashMap<String, String>) -> String {
        build_opening_tag("pre", attributes)
    }

    fn build_code_tag(&self, attributes: &HashMap<String, String>) -> String {
        build_opening_tag("code", attributes)
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

/// Wrapper around the Markdown parser and renderer to render markdown
pub(crate) fn render(text: &str) -> String {
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
                codefence_syntax_highlighter: Some(&CodeAdapter),
            },
        },
    )
}
