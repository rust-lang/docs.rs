use super::{error::Nope, redirect, redirect_base, STATIC_FILE_CACHE_DURATION};
use chrono::Utc;
use iron::{
    headers::CacheDirective,
    headers::{CacheControl, ContentLength, ContentType, LastModified},
    status::Status,
    IronResult, Request, Response, Url,
};
use mime_guess::MimeGuess;
use router::Router;
use std::{ffi::OsStr, fs, path::Path};

const VENDORED_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/vendored.css"));
const STYLE_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/style.css"));
const MENU_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/menu.js"));
const INDEX_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/index.js"));
const STATIC_SEARCH_PATHS: &[&str] = &["vendor/pure-css/css"];

pub(crate) fn static_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let file = cexpect!(req, router.find("file"));

    match file {
        "vendored.css" => serve_resource(VENDORED_CSS, ContentType("text/css".parse().unwrap())),
        "style.css" => serve_resource(STYLE_CSS, ContentType("text/css".parse().unwrap())),
        "index.js" => serve_resource(
            INDEX_JS,
            ContentType("application/javascript".parse().unwrap()),
        ),
        "menu.js" => serve_resource(
            MENU_JS,
            ContentType("application/javascript".parse().unwrap()),
        ),

        file => serve_file(req, file),
    }
}

fn serve_file(req: &Request, file: &str) -> IronResult<Response> {
    // Filter out files that attempt to traverse directories
    if file.contains("..") || file.contains('/') || file.contains('\\') {
        return Err(Nope::ResourceNotFound.into());
    }

    // Find the first path that actually exists
    let path = STATIC_SEARCH_PATHS
        .iter()
        .map(|p| Path::new(p).join(file))
        .find(|p| p.exists())
        .ok_or(Nope::ResourceNotFound)?;
    let contents = ctry!(req, fs::read(&path));

    // If we can detect the file's mime type, set it
    // MimeGuess misses a lot of the file types we need, so there's a small wrapper
    // around it
    let content_type = path
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
        // if we're looking for exactly "favicon.ico", we need to defer to the handler that loads
        // from `public_html`, so return a 404 here to make the main handler carry on
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
    use super::{INDEX_JS, MENU_JS, STATIC_SEARCH_PATHS, STYLE_CSS, VENDORED_CSS};
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
            assert_eq!(resp.content_length().unwrap(), INDEX_JS.len() as u64);
            assert_eq!(resp.text()?, INDEX_JS);

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
            assert_eq!(resp.content_length().unwrap(), MENU_JS.len() as u64);
            assert_eq!(resp.text()?, MENU_JS);

            Ok(())
        });
    }

    #[test]
    fn static_files() {
        wrapper(|env| {
            let web = env.frontend();

            for path in STATIC_SEARCH_PATHS {
                for (file, path) in fs::read_dir(path)?
                    .map(|e| e.unwrap())
                    .map(|e| (e.file_name(), e.path()))
                {
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

            let files = &[("pure-min.css", "text/css")];

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
