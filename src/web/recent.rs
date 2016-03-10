//! Recent crates

use std::collections::BTreeMap;

use iron::prelude::*;
use iron::status;
use handlebars_iron::Template;
use super::{Page, DbConnection, duration_to_str};
use rustc_serialize::json::{Json, ToJson};


struct RecentCrate {
    name: String,
    version: String,
    description: String,
    release_time: String
}

impl ToJson for RecentCrate {
    fn to_json(&self) -> Json {
        let mut tree = BTreeMap::new();
        tree.insert("name".to_string(), self.name.to_json());
        tree.insert("version".to_string(), self.version.to_json());
        tree.insert("description".to_string(), self.description.to_json());
        tree.insert("release_time".to_string(), self.release_time.to_json());
        Json::Object(tree)
    }
}


pub fn recent_crates(req: &mut Request) -> IronResult<Response> {
    let ref conn = *req.extensions.get::<DbConnection>().unwrap();
    let query = "
        SELECT crates.name,
               releases.version,
               releases.description,
               releases.release_time
        FROM releases, crates
        WHERE releases.crate_id = crates.id
        ORDER BY releases.release_time DESC
        LIMIT 50
    ";


    let mut recent_crates: Vec<RecentCrate> = Vec::new();

    for row in &conn.query(query, &[]).unwrap() {
        recent_crates.push(
            RecentCrate {
                name: row.get(0),
                version: row.get(1),
                description: row.get(2),
                release_time: duration_to_str(row.get(3))
            }
        );
    }

    let content = Page::new("Recent crates", recent_crates);
    let mut resp = Response::new();
    resp.set_mut(Template::new("recent", content)).set_mut(status::Ok);

    Ok(resp)
}
