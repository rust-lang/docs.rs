use crate::config::Config;
use axum::{
    http::Request as AxumHttpRequest, middleware::Next, response::Response as AxumResponse,
};
use base64::{engine::general_purpose::STANDARD as b64, Engine};
use std::{
    fmt::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub(crate) struct Csp {
    nonce: String,
    suppress: AtomicBool,
}

impl Csp {
    fn new() -> Self {
        // Nonces need to be different for each single request in order to maintain security, so we
        // generate a new one with a cryptographically-secure generator for each request.
        let mut random = [0u8; 36];
        getrandom::getrandom(&mut random).expect("failed to generate a nonce");

        Self {
            nonce: b64.encode(random),
            suppress: AtomicBool::new(false),
        }
    }

    pub(super) fn suppress(&self, suppress: bool) {
        self.suppress.store(suppress, Ordering::Relaxed);
    }

    pub(super) fn nonce(&self) -> &str {
        &self.nonce
    }

    fn render(&self, content_type: ContentType) -> Option<String> {
        if self.suppress.load(Ordering::Relaxed) {
            return None;
        }
        let mut result = String::new();

        // Disable everything by default
        result.push_str("default-src 'none'");

        // Disable the <base> HTML tag to prevent injected HTML content from changing the base URL
        // of all relative links included in the website.
        result.push_str("; base-uri 'none'");

        // Allow loading images from the same origin. This is added to every response regardless of
        // the MIME type to allow loading favicons.
        //
        // Images from other HTTPS origins are also temporary allowed until issue #66 is fixed.
        result.push_str("; img-src 'self' https:");

        match content_type {
            ContentType::Html => self.render_html(&mut result),
            ContentType::Svg => self.render_svg(&mut result),
            ContentType::Other => {}
        }

        Some(result)
    }

    fn render_html(&self, result: &mut String) {
        // Allow loading any CSS file from the current origin.
        result.push_str("; style-src 'self'");

        // Allow loading any font from the current origin.
        result.push_str("; font-src 'self'");

        // Only allow scripts with the random nonce attached to them.
        //
        // We can't just allow 'self' here, as users can upload arbitrary .js files as part of
        // their documentation and 'self' would allow their execution. Instead, every allowed
        // script must include the random nonce in it, which an attacker is not able to guess.
        //
        // This `.unwrap` is safe since the `Write` impl on str can never fail.
        write!(result, "; script-src 'nonce-{}'", self.nonce).unwrap();
    }

    fn render_svg(&self, result: &mut String) {
        // SVG images are subject to the Content Security Policy, and without a directive allowing
        // style="" inside the file the image will be rendered badly.
        result.push_str("; style-src 'self' 'unsafe-inline'");
    }
}

enum ContentType {
    Html,
    Svg,
    Other,
}

pub(crate) async fn csp_middleware<B>(mut req: AxumHttpRequest<B>, next: Next<B>) -> AxumResponse {
    let csp_report_only = req
        .extensions()
        .get::<Arc<Config>>()
        .expect("missing config extension in request")
        .csp_report_only;

    let csp = Arc::new(Csp::new());
    req.extensions_mut().insert(csp.clone());

    let mut response = next.run(req).await;

    let content_type = response
        .headers()
        .get("Content-Type")
        .map(|header| header.as_bytes());

    let preset = match content_type {
        Some(b"text/html; charset=utf-8") => ContentType::Html,
        Some(b"text/svg+xml") => ContentType::Svg,
        _ => ContentType::Other,
    };

    let rendered = csp.render(preset);

    if let Some(rendered) = rendered {
        let mut headers = response.headers_mut().clone();
        headers.insert(
            // The Report-Only header tells the browser to just log CSP failures instead of
            // actually enforcing them. This is useful to check if the CSP works without
            // impacting production traffic.
            if csp_report_only {
                http::header::CONTENT_SECURITY_POLICY_REPORT_ONLY
            } else {
                http::header::CONTENT_SECURITY_POLICY
            },
            rendered
                .parse()
                .expect("rendered CSP could not be parsed into header value"),
        );
        *response.headers_mut() = headers;
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_nonce() {
        let csp1 = Csp::new();
        let csp2 = Csp::new();
        assert_ne!(csp1.nonce(), csp2.nonce());
    }

    #[test]
    fn test_csp_suppressed() {
        let csp = Csp::new();
        csp.suppress(true);

        assert!(csp.render(ContentType::Other).is_none());
        assert!(csp.render(ContentType::Html).is_none());
        assert!(csp.render(ContentType::Svg).is_none());
    }

    #[test]
    fn test_csp_other() {
        let csp = Csp::new();
        assert_eq!(
            Some("default-src 'none'; base-uri 'none'; img-src 'self' https:".into()),
            csp.render(ContentType::Other)
        );
    }

    #[test]
    fn test_csp_svg() {
        let csp = Csp::new();
        assert_eq!(
            Some(
                "default-src 'none'; base-uri 'none'; img-src 'self' https:; \
                 style-src 'self' 'unsafe-inline'"
                    .into()
            ),
            csp.render(ContentType::Svg)
        );
    }

    #[test]
    fn test_csp_html() {
        let csp = Csp::new();
        assert_eq!(
            Some(format!(
                "default-src 'none'; base-uri 'none'; img-src 'self' https:; \
                 style-src 'self'; font-src 'self'; script-src 'nonce-{}'",
                csp.nonce()
            )),
            csp.render(ContentType::Html)
        );
    }
}
