use crate::error::Result;
use failure::err_msg;
use kuchiki::traits::TendrilSink;
use kuchiki::NodeRef;

/// Extracts the contents of the `<head>` and `<body>` tags from an HTML document, as well as the
/// classes on the `<body>` tag, if any.
pub fn extract_head_and_body(html: &str) -> Result<(String, String, String)> {
    let dom = kuchiki::parse_html().one(html);

    let head = dom
        .select_first("head")
        .map_err(|_| err_msg("couldn't find <head> tag in rustdoc output"))?;
    let body = dom
        .select_first("body")
        .map_err(|_| err_msg("couldn't find <body> tag in rustdoc output"))?;

    let class = body
        .attributes
        .borrow()
        .get("class")
        .map(|v| v.to_owned())
        .unwrap_or_default();

    Ok((serialize(head.as_node()), serialize(body.as_node()), class))
}

fn serialize(v: &NodeRef) -> String {
    let mut contents = Vec::new();
    for child in v.children() {
        child
            .serialize(&mut contents)
            .expect("serialization failed");
    }
    String::from_utf8(contents).expect("non utf-8 html")
}

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
