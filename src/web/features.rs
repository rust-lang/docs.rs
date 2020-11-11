use crate::db::types::Feature;
use crate::{
    db::Pool,
    impl_webpage,
    web::{page::WebPage, MetaData},
};
use iron::{IronResult, Request, Response};
use router::Router;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct FeaturesPage {
    metadata: MetaData,
    features: Option<Vec<Feature>>,
    default_len: usize,
}

impl_webpage! {
    FeaturesPage = "crate/features.html",
}

pub fn build_features_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(req, router.find("name"));
    let version = cexpect!(req, router.find("version"));

    let mut conn = extension!(req, Pool).get()?;
    let rows = ctry!(
        req,
        conn.query(
            "SELECT releases.features FROM releases
            INNER JOIN crates ON crates.id = releases.crate_id
            WHERE crates.name = $1 AND releases.version = $2",
            &[&name, &version]
        )
    );

    let row = cexpect!(req, rows.get(0));

    let mut default_len = 0;
    let features = row
        .get::<'_, usize, Option<Vec<Feature>>>(0)
        .map(|raw| {
            raw.into_iter()
                .filter(|feature| !feature.is_private())
                .map(|feature| (feature.name.clone(), feature))
                .collect::<HashMap<String, Feature>>()
        })
        .map(|mut feature_map| {
            let mut features = get_tree_structure_from_default(&mut feature_map);
            let mut remaining = feature_map
                .into_iter()
                .map(|(_, feature)| feature)
                .collect::<Vec<Feature>>();
            remaining.sort_by_key(|feature| feature.subfeatures.len());

            default_len = features.len();

            features.extend(remaining.into_iter().rev());
            features
        });

    FeaturesPage {
        metadata: cexpect!(req, MetaData::from_crate(&mut conn, &name, &version)),
        features,
        default_len,
    }
    .into_response(req)
}

fn get_tree_structure_from_default(feature_map: &mut HashMap<String, Feature>) -> Vec<Feature> {
    let mut features = Vec::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    queue.push_back("default".into());
    while !queue.is_empty() {
        let name = queue.pop_front().unwrap();
        if let Some(feature) = feature_map.remove(&name) {
            feature
                .subfeatures
                .iter()
                .for_each(|sub| queue.push_back(sub.clone()));
            features.push(feature);
        }
    }
    features
}
