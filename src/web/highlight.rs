use crate::error::Result;
use once_cell::sync::Lazy;
use syntect::{
    html::{ClassStyle, ClassedHTMLGenerator},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

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

pub fn try_with_lang(lang: Option<&str>, code: &str) -> Result<String> {
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

pub fn with_lang(lang: Option<&str>, code: &str) -> String {
    match try_with_lang(lang, code) {
        Ok(highlighted) => highlighted,
        Err(err) => {
            log::error!("failed while highlighting code: {err:?}");
            code.to_owned()
        }
    }
}
