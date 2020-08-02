use crate::web::page::TemplateData;
use lol_html::errors::RewritingError;
use tera::Context;

pub(crate) fn rewrite_lol(
    html: &str,
    ctx: Context,
    templates: &TemplateData,
) -> Result<String, RewritingError> {
    use lol_html::html_content::{ContentType, Element};
    use lol_html::{ElementContentHandlers, RewriteStrSettings};

    let templates = templates.templates.load();
    let tera_head = templates.render("rustdoc/head.html", &ctx).unwrap();
    let tera_body = templates.render("rustdoc/body.html", &ctx).unwrap();

    let head_handler = |head: &mut Element| {
        head.append(&tera_head, ContentType::Html);
        Ok(())
    };
    // Before: <body> ... rustdoc content ... </body>
    // After:
    // ```html
    // <div id="rustdoc_body_wrapper" class="{{ rustdoc_body_class }}" tabindex="-1">
    //      ... rustdoc content ...
    // </div>
    // ```
    let body_handler = |rustdoc_body_class: &mut Element| {
        // Add the `rustdoc` classes to the html body
        rustdoc_body_class.set_attribute("container-rustdoc", "")?;
        rustdoc_body_class.set_attribute("id", "rustdoc_body_wrapper")?;
        rustdoc_body_class.set_attribute("tabindex", "-1")?;
        // Change the `body` to a `div`
        rustdoc_body_class.set_tag_name("div")?;
        // Prepend the tera content
        rustdoc_body_class.prepend(&tera_body, ContentType::Html);
        // Now, make this a full <body> tag
        rustdoc_body_class.before("<body>", ContentType::Html);
        rustdoc_body_class.after("</body>", ContentType::Html);

        Ok(())
    };

    let (head_selector, body_selector) = ("head".parse().unwrap(), "body".parse().unwrap());
    let head = (
        &head_selector,
        ElementContentHandlers::default().element(head_handler),
    );
    let body = (
        &body_selector,
        ElementContentHandlers::default().element(body_handler),
    );
    let settings = RewriteStrSettings {
        element_content_handlers: vec![head, body],
        ..RewriteStrSettings::default()
    };

    lol_html::rewrite_str(html, settings)
}

/*
#[cfg(test)]
mod test {
    #[test]
    fn small_html() {
        let (head, body, class) = super::extract_head_and_body(
            r#"<head><meta name="generator" content="rustdoc"></head><body class="rustdoc struct"><p>hello</p>"#
        ).unwrap();
        assert_eq!(head, r#"<meta content="rustdoc" name="generator">"#);
        assert_eq!(body, "<p>hello</p>");
        assert_eq!(class, "rustdoc struct");
    }

    // more of an integration test
    #[test]
    fn parse_regex_html() {
        let original = std::fs::read_to_string("benches/struct.CaptureMatches.html").unwrap();
        let expected_head = std::fs::read_to_string("tests/regex/head.html").unwrap();
        let expected_body = std::fs::read_to_string("tests/regex/body.html").unwrap();
        let (head, body, class) = super::extract_head_and_body(&original).unwrap();

        assert_eq!(head, expected_head.trim());
        assert_eq!(&body, &expected_body.trim());
        assert_eq!(class, "rustdoc struct");
    }
}
*/
