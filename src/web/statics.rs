use super::{error::Nope, redirect, redirect_base, STATIC_FILE_CACHE_DURATION};
use chrono::Utc;
use iron::{
    headers::CacheDirective,
    headers::{CacheControl, ContentLength, ContentType, LastModified},
    status::Status,
    IronResult, Request, Response, Url,
};
use mime_guess::MimeGuess;
use std::{ffi::OsStr, fs, path::Path};

const VENDORED_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/vendored.css"));
const STYLE_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/style.css"));
const RUSTDOC_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/rustdoc.css"));
const STATIC_SEARCH_PATHS: &[&str] = &["static", "vendor"];

pub(crate) fn static_handler(req: &mut Request) -> IronResult<Response> {
    let mut file = req.url.path();
    file.drain(..2).for_each(std::mem::drop);
    let file = file.join("/");

    Ok(match file.as_str() {
        "vendored.css" => serve_resource(VENDORED_CSS, ContentType("text/css".parse().unwrap()))?,
        "style.css" => serve_resource(STYLE_CSS, ContentType("text/css".parse().unwrap()))?,
        "rustdoc.css" => serve_resource(RUSTDOC_CSS, ContentType("text/css".parse().unwrap()))?,
        file => serve_file(req, file)?,
    })
}

fn serve_file(req: &Request, file: &str) -> IronResult<Response> {
    // Find the first path that actually exists
    let path = STATIC_SEARCH_PATHS
        .iter()
        .filter_map(|root| {
            let path = Path::new(root).join(file);

            // Prevent accessing static files outside the root. This could happen if the path
            // contains `/` or `..`. The check doesn't outright prevent those strings to be present
            // to allow accessing files in subdirectories.
            if path.starts_with(root) {
                Some(path)
            } else {
                None
            }
        })
        .find(|p| p.exists())
        .ok_or(Nope::ResourceNotFound)?;
    let contents = ctry!(req, fs::read(&path));

    // If we can detect the file's mime type, set it
    // MimeGuess misses a lot of the file types we need, so there's a small wrapper
    // around it
    let mut content_type = path
        .extension()
        .and_then(OsStr::to_str)
        .and_then(|ext| match ext {
            "eot" => Some(ContentType(
                "application/vnd.ms-fontobject".parse().unwrap(),
            )),
            "woff2" => Some(ContentType("application/font-woff2".parse().unwrap())),
            "ttf" => Some(ContentType("application/x-font-ttf".parse().unwrap())),

            _ => MimeGuess::from_path(&path)
                .first()
                .map(|mime| ContentType(mime.as_ref().parse().unwrap())),
        });

    if file == "opensearch.xml" {
        content_type = Some(ContentType(
            "application/opensearchdescription+xml".parse().unwrap(),
        ));
    }

    serve_resource(contents, content_type)
}

fn serve_resource<R, C>(resource: R, content_type: C) -> IronResult<Response>
where
    R: AsRef<[u8]>,
    C: Into<Option<ContentType>>,
{
    let mut response = Response::with((Status::Ok, resource.as_ref()));

    let cache = vec![
        CacheDirective::Public,
        CacheDirective::MaxAge(STATIC_FILE_CACHE_DURATION as u32),
    ];
    response.headers.set(CacheControl(cache));

    response
        .headers
        .set(ContentLength(resource.as_ref().len() as u64));
    response.headers.set(LastModified(
        Utc::now()
            .format("%a, %d %b %Y %T %Z")
            .to_string()
            .parse()
            .unwrap(),
    ));

    if let Some(content_type) = content_type.into() {
        response.headers.set(content_type);
    }

    Ok(response)
}

pub(super) fn ico_handler(req: &mut Request) -> IronResult<Response> {
    if let Some(&"favicon.ico") = req.url.path().last() {
        // if we're looking for exactly "favicon.ico", we need to defer to the handler that
        // actually serves it, so return a 404 here to make the main handler carry on
        Err(Nope::ResourceNotFound.into())
    } else {
        // if we're looking for something like "favicon-20190317-1.35.0-nightly-c82834e2b.ico",
        // redirect to the plain one so that the above branch can trigger with the correct filename
        let url = ctry!(
            req,
            Url::parse(&format!("{}/favicon.ico", redirect_base(req))),
        );

        Ok(redirect(url))
    }
}

#[cfg(test)]
mod tests {
    use super::{STATIC_SEARCH_PATHS, STYLE_CSS, VENDORED_CSS};
    use crate::test::wrapper;
    use std::fs;

    #[test]
    fn style_css() {
        wrapper(|env| {
            let web = env.frontend();

            let resp = web.get("/-/static/style.css").send()?;
            assert!(resp.status().is_success());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"text/css".parse().unwrap()),
            );
            assert_eq!(resp.content_length().unwrap(), STYLE_CSS.len() as u64);
            assert_eq!(resp.text()?, STYLE_CSS);

            Ok(())
        });
    }

    #[test]
    fn vendored_css() {
        wrapper(|env| {
            let web = env.frontend();

            let resp = web.get("/-/static/vendored.css").send()?;
            assert!(resp.status().is_success());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"text/css".parse().unwrap()),
            );
            assert_eq!(resp.content_length().unwrap(), VENDORED_CSS.len() as u64);
            assert_eq!(resp.text()?, VENDORED_CSS);

            Ok(())
        });
    }

    #[test]
    fn index_js() {
        wrapper(|env| {
            let web = env.frontend();

            let resp = web.get("/-/static/index.js").send()?;
            assert!(resp.status().is_success());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"application/javascript".parse().unwrap()),
            );
            assert!(resp.content_length().unwrap() > 10);
            assert!(resp.text()?.contains("copyTextHandler"));

            Ok(())
        });
    }

    #[test]
    fn menu_js() {
        wrapper(|env| {
            let web = env.frontend();

            let resp = web.get("/-/static/menu.js").send()?;
            assert!(resp.status().is_success());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"application/javascript".parse().unwrap()),
            );
            assert!(resp.content_length().unwrap() > 10);
            assert!(resp.text()?.contains("closeMenu"));

            Ok(())
        });
    }

    #[test]
    fn static_files() {
        wrapper(|env| {
            let web = env.frontend();

            for root in STATIC_SEARCH_PATHS {
                for entry in walkdir::WalkDir::new(root) {
                    let entry = entry?;
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let file = entry.path().strip_prefix(root).unwrap();
                    let path = entry.path();

                    let url = format!("/-/static/{}", file.to_str().unwrap());
                    let resp = web.get(&url).send()?;

                    assert!(resp.status().is_success(), "failed to fetch {:?}", url);
                    assert_eq!(
                        resp.bytes()?,
                        fs::read(&path).unwrap(),
                        "failed to fetch {:?}",
                        url,
                    );
                }
            }

            Ok(())
        });
    }

    #[test]
    fn static_file_that_doesnt_exist() {
        wrapper(|env| {
            let web = env.frontend();
            assert_eq!(
                web.get("/-/static/whoop-de-do.png")
                    .send()?
                    .status()
                    .as_u16(),
                404,
            );

            Ok(())
        });
    }

    #[test]
    fn static_mime_types() {
        wrapper(|env| {
            let web = env.frontend();

            let files = &[("highlightjs/styles/dark.min.css", "text/css")];

            for (file, mime) in files {
                let url = format!("/-/static/{}", file);
                let resp = web.get(&url).send()?;

                assert_eq!(
                    resp.headers().get("Content-Type"),
                    Some(&mime.parse().unwrap()),
                    "{:?} has an incorrect content type",
                    url,
                );
            }

            Ok(())
        });
    }

    #[test]
    fn directory_traversal() {
        wrapper(|env| {
            let web = env.frontend();

            let urls = &[
                "../LICENSE.txt",
                "%2e%2e%2fLICENSE.txt",
                "%2e%2e/LICENSE.txt",
                "..%2fLICENSE.txt",
                "%2e%2e%5cLICENSE.txt",
            ];

            for url in urls {
                let req = web.get(&format!("/-/static/{}", url)).send()?;
                assert_eq!(req.status().as_u16(), 404);
            }

            Ok(())
        });
    }
}
