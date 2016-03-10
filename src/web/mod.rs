

mod recent;

use std::path::Path;
use std::collections::BTreeMap;

use ::db;

use postgres;
use iron::prelude::*;
use iron::{BeforeMiddleware, typemap};
use router::Router;
use mount::Mount;
use staticfile::Static;
use handlebars_iron::{HandlebarsEngine, DirectorySource};
use rustc_serialize::json::{Json, ToJson};
use time;



struct Page<T: ToJson> {
    title: String,
    content: T
}


impl<T: ToJson> ToJson for Page<T> {
    fn to_json(&self) -> Json {
        let mut tree = BTreeMap::new();
        tree.insert("title".to_string(), self.title.to_json());
        tree.insert("content".to_string(), self.content.to_json());
        Json::Object(tree)
    }
}


impl<T: ToJson> Page<T> {
    fn new(title: &str, content: T) -> Page<T> {
        Page {
            title: title.to_string(),
            content: content
        }
    }
}



// Database connection BeforeMiddleware filter
struct DbConnection;


impl typemap::Key for DbConnection { type Value = postgres::Connection; }


impl BeforeMiddleware for DbConnection {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions.insert::<DbConnection>(db::connect_db().unwrap());
        Ok(())
    }
}



fn duration_to_str(ts: time::Timespec) -> String {

    let tm = time::at(ts);
    let delta = time::now() - tm;

    if delta.num_days() > 5 {
        format!("{}", tm.strftime("%b %d, %Y").unwrap())
    } else if delta.num_days() > 1 {
        format!("{} days ago", delta.num_days())
    } else if delta.num_days() == 1 {
        "one day ago".to_string()
    } else if delta.num_hours() > 1 {
        format!("{} hours ago", delta.num_hours())
    } else if delta.num_hours() == 1 {
        "an hour ago".to_string()
    } else if delta.num_minutes() > 1 {
        format!("{} minutes ago", delta.num_minutes())
    } else if delta.num_minutes() == 1 {
        "one minute ago".to_string()
    } else if delta.num_seconds() > 0 {
        format!("{} seconds ago", delta.num_seconds())
    } else {
        "just now".to_string()
    }

}



/// Starts main web application of cratesfyi on localhost:3000
pub fn start_cratesfyi_server() {

    // router
    let mut router = Router::new();
    router.get("/recent", recent::recent_crates);

    // templates
    let mut hbse = HandlebarsEngine::new2();
    hbse.add(Box::new(DirectorySource::new("./templates/", ".hbs")));

    if let Err(e) = hbse.reload() {
        panic!("{:#?}", e);
    }

    // router chain for db and hbs stuff
    let mut router_chain = Chain::new(router);
    router_chain.link_before(DbConnection);
    router_chain.link_after(hbse);

    // mount for static files
    let mut mount = Mount::new();
    mount
        .mount("/", router_chain)
        .mount("/static", Static::new(Path::new("templates/raw")));


    println!("cratesfyi started on http://localhost:3000/");
    Iron::new(mount).http("localhost:3000").unwrap();
}
