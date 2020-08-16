use super::error::Nope;
use iron::{
    headers::CacheDirective,
    headers::{CacheControl, ContentType},
    status::Status,
    IronError, IronResult, Request, Response,
};
use router::Router;

const EOT: &str = "application/vnd.ms-fontobject";
const TTF: &str = "application/x-font-ttf";
const WOFF2: &str = "application/font-woff2";
const SVG: &str = "image/svg+xml";
const WOFF: &str = "application/font-woff";

macro_rules! serve_static_files {
    ($($file_name:literal => $content_type:expr),* $(,)?) => {
        // Generate the static file handler
        pub fn static_handler(req: &mut Request) -> IronResult<Response> {
            let router = extension!(req, Router);
            let file = cexpect!(req, router.find("file"));

            // Select which file the user requested or return an error if it doesn't exist
            let (contents, content_type): (&'static [u8], ContentType) = match file {
                $(
                    $file_name => (
                        include_bytes!(concat!("../../vendor/fontawesome/webfonts/", $file_name)),
                        ContentType($content_type.parse().unwrap()),
                    ),
                )*

                _ => return Err(IronError::new(Nope::ResourceNotFound, Status::NotFound)),
            };

            // Set the cache times
            let mut response = Response::with((Status::Ok, contents));
            let cache = vec![
                CacheDirective::Public,
                CacheDirective::MaxAge(super::STATIC_FILE_CACHE_DURATION as u32),
            ];
            response.headers.set(content_type);
            response.headers.set(CacheControl(cache));

            Ok(response)
        }

        // Test each static route we serve
        #[test]
        fn serve_static_files() {
            $crate::test::wrapper(|env| {
                let web = env.frontend();

                $(
                    assert!(
                        web.get(concat!("/-/static/", $file_name)).send()?.status().is_success(),
                        concat!("failed while requesting a static file: '/-/static/", $file_name, "'"),
                    );
                )*

                Ok(())
            });
        }
    };
}

// Serve all of our vendor font & svg files
serve_static_files! {
    "fa-brands-400.eot"    => EOT,
    "fa-brands-400.ttf"    => TTF,
    "fa-brands-400.woff2"  => WOFF2,
    "fa-regular-400.svg"   => SVG,
    "fa-regular-400.woff"  => WOFF,
    "fa-solid-900.eot"     => EOT,
    "fa-solid-900.ttf"     => TTF,
    "fa-solid-900.woff2"   => WOFF2,
    "fa-brands-400.svg"    => SVG,
    "fa-brands-400.woff"   => WOFF,
    "fa-regular-400.eot"   => EOT,
    "fa-regular-400.ttf"   => TTF,
    "fa-regular-400.woff2" => WOFF2,
    "fa-solid-900.svg"     => SVG,
    "fa-solid-900.woff"    => WOFF,
}

#[test]
fn static_file_that_doesnt_exist() {
    crate::test::wrapper(|env| {
        let web = env.frontend();
        assert_eq!(web
            .get("/-/static/whoop-de-do.png")
            .send()?
            .status()
            .as_u16(), 404);

        Ok(())
    });
}
