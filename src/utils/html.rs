use error::Result;
use failure::err_msg;

use html5ever::serialize::{serialize, SerializeOpts};
use html5ever::rcdom::{RcDom, NodeData, Handle};
use html5ever::driver::{parse_document, ParseOpts};
use html5ever::tendril::TendrilSink;

/// Extracts the contents of the `<head>` and `<body>` tags from an HTML document, as well as the
/// classes on the `<body>` tag, if any.
pub fn extract_head_and_body(html: &str) -> Result<(String, String, String)> {
    let parser = parse_document(RcDom::default(), ParseOpts::default());
    let dom = parser.one(html);

    let (head, body) = extract_from_rcdom(&dom)?;
    let class = extract_class(&body);

    Ok((stringify(head), stringify(body), class))
}

fn extract_from_rcdom(dom: &RcDom) -> Result<(Handle, Handle)> {
    let mut worklist = vec![dom.document.clone()];
    let (mut head, mut body) = (None, None);

    while let Some(handle) = worklist.pop() {
        match handle.data {
            NodeData::Element { ref name, .. } => match name.local.as_ref() {
                "head" => {
                    if head.is_some() {
                        return Err(err_msg("duplicate <head> tag"));
                    } else {
                        head = Some(handle.clone());
                    }
                }
                "body" => {
                    if body.is_some() {
                        return Err(err_msg("duplicate <body> tag"));
                    } else {
                        body = Some(handle.clone());
                    }
                }
                _ => {}  // do nothing
            }
            _ => {}  // do nothing
        }

        worklist.extend(handle.children.borrow().iter().cloned());
    }

    let head = head.ok_or_else(|| err_msg("couldn't find <head> tag in rustdoc output"))?;
    let body = body.ok_or_else(|| err_msg("couldn't find <body> tag in rustdoc output"))?;
    Ok((head, body))
}

fn stringify(node: Handle) -> String {
    let mut vec = Vec::new();
    serialize(&mut vec, &node, SerializeOpts::default())
        .expect("serializing into buffer failed");

    String::from_utf8(vec).expect("html5ever returned non-utf8 data")
}

fn extract_class(node: &Handle) -> String {
    match node.data {
        NodeData::Element { ref attrs, .. } => {
            let attrs = attrs.borrow();

            attrs.iter()
                 .find(|a| &a.name.local == "class")
                 .map_or(String::new(), |a| a.value.to_string())
        }
        _ => String::new()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn round_trip(a: &str) -> String {
        let parser = parse_document(RcDom::default(), ParseOpts::default());
        stringify(parser.one(a).document)
    }
    fn mostly_equal(a: &str, b: &str) -> bool {
        round_trip(a.trim()).trim() == round_trip(b.trim()).trim()
    }

    #[test]
    fn small_html() {
        let (head, body, class) = super::extract_head_and_body(
            r#"<head><meta name="generator" content="rustdoc"></head><body class="rustdoc struct"><p>hello</p>"#
        ).unwrap();
        assert_eq!(head, r#"<meta name="generator" content="rustdoc">"#);
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
        assert!(mostly_equal(&head, &expected_head));
        assert!(mostly_equal(&body, &expected_body));
        assert_eq!(class, "rustdoc struct");
    }
}
