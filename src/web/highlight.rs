use crate::error::Result;
use once_cell::sync::Lazy;
use syntect::{
    html::{ClassStyle, ClassedHTMLGenerator},
    parsing::{SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

const TOTAL_CODE_BYTE_LENGTH_LIMIT: usize = 2 * 1024 * 1024;
const PER_LINE_BYTE_LENGTH_LIMIT: usize = 512;

#[derive(Debug, thiserror::Error)]
#[error("the code exceeded a highlighting limit")]
pub struct LimitsExceeded;

static SYNTAXES: Lazy<SyntaxSet> = Lazy::new(|| {
    static SYNTAX_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/syntect.packdump"));

    let syntaxes: SyntaxSet = syntect::dumps::from_uncompressed_data(SYNTAX_DATA).unwrap();

    let names = syntaxes
        .syntaxes()
        .iter()
        .map(|s| &s.name)
        .collect::<Vec<_>>();
    log::debug!("known syntaxes {names:?}");

    syntaxes
});

fn try_with_syntax(syntax: &SyntaxReference, code: &str) -> Result<String> {
    if code.len() > TOTAL_CODE_BYTE_LENGTH_LIMIT {
        return Err(LimitsExceeded.into());
    }

    let mut html_generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        &SYNTAXES,
        ClassStyle::SpacedPrefixed { prefix: "syntax-" },
    );

    for line in LinesWithEndings::from(code) {
        if line.len() > PER_LINE_BYTE_LENGTH_LIMIT {
            return Err(LimitsExceeded.into());
        }
        html_generator.parse_html_for_line_which_includes_newline(line)?;
    }

    Ok(html_generator.finalize())
}

fn select_syntax(name: Option<&str>, code: &str) -> &'static SyntaxReference {
    name.and_then(|name| {
        SYNTAXES.find_syntax_by_token(name).or_else(|| {
            name.rsplit_once('.')
                .and_then(|(_, ext)| SYNTAXES.find_syntax_by_token(ext))
        })
    })
    .or_else(|| SYNTAXES.find_syntax_by_first_line(code))
    .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text())
}

pub fn try_with_lang(lang: Option<&str>, code: &str) -> Result<String> {
    try_with_syntax(select_syntax(lang, code), code)
}

pub fn with_lang(lang: Option<&str>, code: &str) -> String {
    match try_with_lang(lang, code) {
        Ok(highlighted) => highlighted,
        Err(err) => {
            if err.is::<LimitsExceeded>() {
                log::debug!("hit limit while highlighting code");
            } else {
                log::error!("failed while highlighting code: {err:?}");
            }
            crate::web::page::templates::filters::escape_html(code)
                .map(|s| s.to_string())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        select_syntax, try_with_lang, with_lang, LimitsExceeded, PER_LINE_BYTE_LENGTH_LIMIT,
        TOTAL_CODE_BYTE_LENGTH_LIMIT,
    };

    #[test]
    fn custom_filetypes() {
        let toml = select_syntax(Some("toml"), "");

        assert_eq!(select_syntax(Some("Cargo.toml.orig"), "").name, toml.name);
        assert_eq!(select_syntax(Some("Cargo.lock"), "").name, toml.name);
    }

    #[test]
    fn dotfile_with_extension() {
        let toml = select_syntax(Some("toml"), "");

        assert_eq!(select_syntax(Some(".rustfmt.toml"), "").name, toml.name);
    }

    #[test]
    fn limits() {
        let is_limited = |s: String| {
            try_with_lang(Some("toml"), &s)
                .unwrap_err()
                .is::<LimitsExceeded>()
        };
        assert!(is_limited("a\n".repeat(TOTAL_CODE_BYTE_LENGTH_LIMIT)));
        assert!(is_limited("aa".repeat(PER_LINE_BYTE_LENGTH_LIMIT)));
    }

    #[test]
    fn limited_escaped() {
        let text = "<p>\n".to_string() + "aa".repeat(PER_LINE_BYTE_LENGTH_LIMIT).as_str();
        let highlighted = with_lang(Some("toml"), &text);
        assert!(highlighted.starts_with("&lt;p&gt;\n"));
    }
}
