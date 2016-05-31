//! Simple web server to handle rustdoc.crates.fyi


use std::path::Path;

use iron::prelude::*;
use iron::{status, Url, Handler};
use iron::modifiers::Redirect;
use staticfile::Static;
use semver::{Version, VersionReq};
use regex::Regex;
use std::time::Duration;
use db::connect_db;



struct Redirector {
    static_handler: Box<Handler>,
}


impl Redirector {
    pub fn new<P: AsRef<Path>>(documentations_path: P) -> Redirector {
        let static_handler = Static::new(documentations_path.as_ref())
                                 .cache(Duration::from_secs(60 * 60 * 24 * 3));
        Redirector { static_handler: Box::new(static_handler) }
    }


    fn redirect_to_doc(&self,
                       req: &Request,
                       name: &str,
                       vers: &str,
                       target_name: &str)
                       -> IronResult<Response> {
        let url = Url::parse(&format!("{}://{}/{}/{}/{}/",
                                      req.url.scheme,
                                      req.url.host,
                                      name,
                                      vers,
                                      target_name)[..])
                      .unwrap();
        let mut resp = Response::with((status::Found, Redirect(url.clone())));

        use iron::headers::{Expires, HttpDate};
        use time;
        resp.headers.set(Expires(HttpDate(time::now())));

        Ok(resp)
    }


    fn check_crate_redirection(&self,
                               req: &Request,
                               crate_name: &str,
                               version: Option<&str>)
                               -> IronResult<Response> {
        let req_version = version.unwrap_or("*");

        let conn = connect_db().unwrap();

        let mut versions = Vec::new();

        // get every version of a crate
        for row in &conn.query("SELECT version, target_name FROM crates,releases WHERE \
                                crates.name = $1 AND crates.id = releases.crate_id",
                               &[&crate_name])
                        .unwrap() {
            let version: String = row.get(0);
            let target_name: String = row.get(1);
            versions.push((version, target_name));
        }

        // first check for exact match
        // we can't expect users to use semver in query
        for version in &versions {
            if version.0 == *req_version {
                return self.redirect_to_doc(&req, crate_name, req_version, &version.1[..]);
            }
        }

        // Now try to match with semver
        let req_sem_ver = VersionReq::parse(req_version).unwrap();
        for version in &versions {
            let sem_ver = Version::parse(&version.0[..]).unwrap();
            if req_sem_ver.matches(&sem_ver) {
                return self.redirect_to_doc(&req, crate_name, &version.0[..], &version.1[..]);
            }
        }

        Ok(Response::with((status::NotFound, "Crate not found")))
    }
}

impl Handler for Redirector {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        let re = Regex::new(r"^([\w_-]+)/*([\w.-]*)/*$").unwrap();
        let path = req.url.path.join("/");

        match re.captures(&path[..]) {
            Some(caps) => self.check_crate_redirection(req, caps.at(1).unwrap(), caps.at(2)),
            None => self.static_handler.handle(req),
        }
    }
}



/// Starts rustdoc web server
pub fn start_rustdoc_web_server<P: AsRef<Path>>(documentations_path: P, port: u16) {
    let redirector = Redirector::new(documentations_path.as_ref());
    info!("rustdoc web server starting on http://localhost:{}/", port);
    Iron::new(redirector).http(("localhost", port)).unwrap();
}


#[cfg(test)]
mod test {
    extern crate env_logger;
    use super::*;
    use std::path::Path;

    #[test]
    #[ignore]
    fn test_start_rustdoc_web_server() {
        // FIXME: This test is doing nothing
        let _ = env_logger::init();
        start_rustdoc_web_server(Path::new("../cratesfyi-prefix/documentations"), 3000);
    }
}
