//! Source code browser

use crate::{
    db::Pool,
    web::{cache::CachePolicy, match_version, redirect_base, MatchSemver, Url},
};
use iron::{IronResult, Request, Response};
use router::Router;

pub fn source_browser_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let mut crate_name = cexpect!(req, router.find("name"));
    let req_version = cexpect!(req, router.find("version"));
    let pool = extension!(req, Pool);
    let mut conn = pool.get()?;

    let mut req_path = req.url.path();
    // remove first elements from path which is /crate/:name/:version/source
    req_path.drain(0..4);

    let v = match_version(&mut conn, crate_name, Some(req_version))?;
    if let Some(new_name) = &v.corrected_name {
        // `match_version` checked against -/_ typos, so if we have a name here we should
        // use that instead
        crate_name = new_name;
    }

    let version = match v.version {
        MatchSemver::Latest((version, _)) => version,
        MatchSemver::Exact((version, _)) => version,
        MatchSemver::Semver((version, _)) => {
            let url = ctry!(
                req,
                Url::parse(&format!(
                    "{}/crate/{}/{}/source/{}",
                    redirect_base(req),
                    crate_name,
                    version,
                    req_path.join("/"),
                )),
            );

            return Ok(super::cached_redirect(url, CachePolicy::ForeverInCdn));
        }
    };

    let file_path = {
        let mut req_path = req.url.path();
        // remove first elements from path which is /crate/:name/:version/source
        for _ in 0..4 {
            req_path.remove(0);
        }
        req_path.join("/")
    };

    let url = ctry!(
        req,
        Url::parse(&format!(
            "https://sourcegraph.com/crates/{crate_name}@v{version}/-/blob/{file_path}"
        ))
    );
    Ok(super::redirect(url))
}

#[cfg(test)]
mod tests {
    use crate::test::*;
    use crate::web::cache::CachePolicy;
    use test_case::test_case;

    #[test_case(true)]
    #[test_case(false)]
    fn semver_handled(archive_storage: bool) {
        wrapper(|env| {
            env.fake_release()
                .archive_storage(archive_storage)
                .name("mbedtls")
                .version("0.2.0")
                .source_file("README.md", b"hello")
                .create()?;
            let web = env.frontend();
            assert_success("/crate/mbedtls/0.2.0/source/", web)?;
            assert_redirect_cached_unchecked(
                "/crate/mbedtls/*/source/",
                "/crate/mbedtls/0.2.0/source/",
                CachePolicy::ForeverInCdn,
                web,
                &env.config(),
            )?;
            Ok(())
        })
    }
}
