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
        let mut tmp;
        let klass = if let Some(classes) = rustdoc_body_class.get_attribute("class") {
            tmp = classes;
            tmp.push_str(" container-rustdoc");
            &tmp
        } else {
            "container-rustdoc"
        };
        rustdoc_body_class.set_attribute("class", klass)?;
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
