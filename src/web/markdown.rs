use crate::web::highlight;
use comrak::{
    adapters::SyntaxHighlighterAdapter, ExtensionOptions, Options, Plugins, RenderPlugins,
};
use std::collections::HashMap;

#[derive(Debug)]
struct CodeAdapter<F>(F);

impl<F: Fn(Option<&str>, &str) -> String + Send + Sync> SyntaxHighlighterAdapter
    for CodeAdapter<F>
{
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
    highlighter: impl Fn(Option<&str>, &str) -> String + Send + Sync,
) -> String {
    let mut extension = ExtensionOptions::default();
    extension.superscript = true;
    extension.table = true;
    extension.autolink = true;
    extension.tasklist = true;
    extension.strikethrough = true;

    let options = Options {
        extension,
        ..Default::default()
    };

    let code_adapter = CodeAdapter(highlighter);

    let mut render = RenderPlugins::default();
    render.codefence_syntax_highlighter = Some(&code_adapter);

    let mut plugins = Plugins::default();
    plugins.render = render;

    comrak::markdown_to_html_with_plugins(text, &options, &plugins)
}

/// Wrapper around the Markdown parser and renderer to render markdown
pub fn render(text: &str) -> String {
    render_with_highlighter(text, highlight::with_lang)
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
            |lang, code| {
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
