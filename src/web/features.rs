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

const DEFAULT_NAME: &str = "default";

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

    let mut features = None;
    let mut default_len = 0;

    if let Some(raw) = row.get(0) {
        let result = order_features_and_count_default_len(raw);
        features = Some(result.0);
        default_len = result.1;
    }

    FeaturesPage {
        metadata: cexpect!(req, MetaData::from_crate(&mut conn, &name, &version)),
        features,
        default_len,
    }
    .into_response(req)
}

fn order_features_and_count_default_len(raw: Vec<Feature>) -> (Vec<Feature>, usize) {
    let mut feature_map = get_feature_map(raw);
    let mut features = get_tree_structure_from_default(&mut feature_map);
    let mut remaining: Vec<_> = feature_map
        .into_iter()
        .map(|(_, feature)| feature)
        .collect();
    remaining.sort_by_key(|feature| feature.subfeatures.len());

    let default_len = features.len();

    features.extend(remaining.into_iter().rev());
    (features, default_len)
}

fn get_tree_structure_from_default(feature_map: &mut HashMap<String, Feature>) -> Vec<Feature> {
    let mut features = Vec::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    queue.push_back(DEFAULT_NAME.into());
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

fn get_feature_map(raw: Vec<Feature>) -> HashMap<String, Feature> {
    raw.into_iter()
        .filter(|feature| !feature.is_private())
        .map(|feature| (feature.name.clone(), feature))
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::db::types::Feature;
    use crate::web::features::{
        get_feature_map, get_tree_structure_from_default, order_features_and_count_default_len,
        DEFAULT_NAME,
    };

    #[test]
    fn test_feature_map_filters_private() {
        let private1 = Feature::new("_private1".into(), vec!["feature1".into()]);
        let feature2 = Feature::new("feature2".into(), Vec::new());

        let raw = vec![private1.clone(), feature2.clone()];
        let feature_map = get_feature_map(raw);

        assert_eq!(feature_map.len(), 1);
        assert!(feature_map.contains_key(&feature2.name));
        assert!(!feature_map.contains_key(&private1.name));
    }

    #[test]
    fn test_default_tree_structure_with_nested_default() {
        let default = Feature::new(DEFAULT_NAME.into(), vec!["feature1".into()]);
        let non_default = Feature::new("non-default".into(), Vec::new());
        let feature1 = Feature::new(
            "feature1".into(),
            vec!["feature2".into(), "feature3".into()],
        );
        let feature2 = Feature::new("feature2".into(), Vec::new());
        let feature3 = Feature::new("feature3".into(), Vec::new());

        let raw = vec![
            default.clone(),
            non_default.clone(),
            feature3.clone(),
            feature2.clone(),
            feature1.clone(),
        ];
        let mut feature_map = get_feature_map(raw);
        let default_tree = get_tree_structure_from_default(&mut feature_map);

        assert_eq!(feature_map.len(), 1);
        assert_eq!(default_tree.len(), 4);
        assert!(feature_map.contains_key(&non_default.name));
        assert!(!feature_map.contains_key(&default.name));
        assert_eq!(default_tree[0], default);
        assert_eq!(default_tree[1], feature1);
        assert_eq!(default_tree[2], feature2);
        assert_eq!(default_tree[3], feature3);
    }

    #[test]
    fn test_default_tree_structure_without_default() {
        let feature1 = Feature::new(
            "feature1".into(),
            vec!["feature2".into(), "feature3".into()],
        );
        let feature2 = Feature::new("feature2".into(), Vec::new());
        let feature3 = Feature::new("feature3".into(), Vec::new());

        let raw = vec![feature3.clone(), feature2.clone(), feature1.clone()];
        let mut feature_map = get_feature_map(raw);
        let default_tree = get_tree_structure_from_default(&mut feature_map);

        assert_eq!(feature_map.len(), 3);
        assert_eq!(default_tree.len(), 0);
        assert!(feature_map.contains_key(&feature1.name));
        assert!(feature_map.contains_key(&feature2.name));
        assert!(feature_map.contains_key(&feature3.name));
    }

    #[test]
    fn test_default_tree_structure_single_default() {
        let default = Feature::new(DEFAULT_NAME.into(), Vec::new());
        let non_default = Feature::new("non-default".into(), Vec::new());

        let raw = vec![default.clone(), non_default.clone()];
        let mut feature_map = get_feature_map(raw);
        let default_tree = get_tree_structure_from_default(&mut feature_map);

        assert_eq!(feature_map.len(), 1);
        assert_eq!(default_tree.len(), 1);
        assert!(feature_map.contains_key(&non_default.name));
        assert!(!feature_map.contains_key(&default.name));
        assert_eq!(default_tree[0], default);
    }

    #[test]
    fn test_order_features_and_get_len_without_default() {
        let feature1 = Feature::new(
            "feature1".into(),
            vec!["feature10".into(), "feature11".into()],
        );
        let feature2 = Feature::new("feature2".into(), vec!["feature20".into()]);
        let feature3 = Feature::new("feature3".into(), Vec::new());

        let raw = vec![feature3.clone(), feature2.clone(), feature1.clone()];
        let (features, default_len) = order_features_and_count_default_len(raw);

        assert_eq!(features.len(), 3);
        assert_eq!(default_len, 0);
        assert_eq!(features[0], feature1);
        assert_eq!(features[1], feature2);
        assert_eq!(features[2], feature3);
    }

    #[test]
    fn test_order_features_and_get_len_single_default() {
        let default = Feature::new(DEFAULT_NAME.into(), Vec::new());
        let non_default = Feature::new("non-default".into(), Vec::new());

        let raw = vec![default.clone(), non_default.clone()];
        let (features, default_len) = order_features_and_count_default_len(raw);

        assert_eq!(features.len(), 2);
        assert_eq!(default_len, 1);
        assert_eq!(features[0], default);
        assert_eq!(features[1], non_default);
    }
}
