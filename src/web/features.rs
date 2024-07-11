use crate::{
    db::types::Feature,
    impl_axum_webpage,
    web::{
        cache::CachePolicy,
        crate_details::CrateDetails,
        error::{AxumNope, AxumResult},
        extractors::{DbConnection, Path},
        filters,
        headers::CanonicalUrl,
        match_version, MetaData, ReqVersion,
    },
};
use anyhow::anyhow;
use axum::response::IntoResponse;
use rinja::Template;
use std::collections::{HashMap, VecDeque};

const DEFAULT_NAME: &str = "default";

#[derive(Template)]
#[template(path = "crate/features.html")]
#[derive(Debug, Clone)]
struct FeaturesPage {
    metadata: MetaData,
    features: Option<Vec<Feature>>,
    default_len: usize,
    canonical_url: CanonicalUrl,
    is_latest_url: bool,
    csp_nonce: String,
}

impl_axum_webpage! {
    FeaturesPage,
    cache_policy = |page| if page.is_latest_url {
        CachePolicy::ForeverInCdn
    } else {
        CachePolicy::ForeverInCdnAndStaleInBrowser
    },
}

impl FeaturesPage {
    pub(crate) fn krate(&self) -> Option<&CrateDetails> {
        None
    }
    pub(crate) fn permalink_path(&self) -> &str {
        ""
    }
    pub(crate) fn get_metadata(&self) -> Option<&MetaData> {
        Some(&self.metadata)
    }
    pub(crate) fn use_direct_platform_links(&self) -> bool {
        true
    }
}

pub(crate) async fn build_features_handler(
    Path((name, req_version)): Path<(String, ReqVersion)>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    let version = match_version(&mut conn, &name, &req_version)
        .await?
        .assume_exact_name()?
        .into_canonical_req_version_or_else(|version| {
            AxumNope::Redirect(
                format!("/crate/{}/{}/features", &name, version),
                CachePolicy::ForeverInCdn,
            )
        })?
        .into_version();

    let metadata =
        MetaData::from_crate(&mut conn, &name, &version, Some(req_version.clone())).await?;

    let row = sqlx::query!(
        r#"
        SELECT releases.features as "features?: Vec<Feature>"
        FROM releases
        INNER JOIN crates ON crates.id = releases.crate_id
        WHERE crates.name = $1 AND releases.version = $2"#,
        name,
        version.to_string(),
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow!("missing release"))?;

    let mut features = None;
    let mut default_len = 0;

    if let Some(raw_features) = row.features {
        let result = order_features_and_count_default_len(raw_features);
        features = Some(result.0);
        default_len = result.1;
    }

    Ok(FeaturesPage {
        metadata,
        features,
        default_len,
        is_latest_url: req_version.is_latest(),
        canonical_url: CanonicalUrl::from_path(format!("/crate/{}/latest/features", &name)),
        csp_nonce: String::new(),
    }
    .into_response())
}

fn order_features_and_count_default_len(raw: Vec<Feature>) -> (Vec<Feature>, usize) {
    let mut feature_map = get_feature_map(raw);
    let mut features = get_tree_structure_from_default(&mut feature_map);
    let mut remaining = Vec::from_iter(feature_map.into_values());
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
    use super::*;
    use crate::test::{assert_cache_control, assert_redirect_cached, wrapper};
    use reqwest::StatusCode;

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

    #[test]
    fn semver_redirect() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.2.1")
                .features(HashMap::new())
                .create()?;

            assert_redirect_cached(
                "/crate/foo/~0.2/features",
                "/crate/foo/0.2.1/features",
                CachePolicy::ForeverInCdn,
                env.frontend(),
                &env.config(),
            )?;
            Ok(())
        });
    }

    #[test]
    fn specific_version_correctly_cached() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.2.0")
                .features(HashMap::new())
                .create()?;

            let resp = env.frontend().get("/crate/foo/0.2.0/features").send()?;
            assert!(resp.status().is_success());
            assert_cache_control(
                &resp,
                CachePolicy::ForeverInCdnAndStaleInBrowser,
                &env.config(),
            );
            Ok(())
        });
    }

    #[test]
    fn latest_200() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .features(HashMap::new())
                .create()?;

            env.fake_release()
                .name("foo")
                .version("0.2.0")
                .features(HashMap::new())
                .create()?;

            let resp = env.frontend().get("/crate/foo/latest/features").send()?;
            assert!(resp.status().is_success());
            assert_cache_control(&resp, CachePolicy::ForeverInCdn, &env.config());
            assert!(resp.url().as_str().ends_with("/crate/foo/latest/features"));
            let body = String::from_utf8(resp.bytes().unwrap().to_vec()).unwrap();
            assert!(body.contains("<a href=\"/crate/foo/latest/builds\""));
            assert!(body.contains("<a href=\"/crate/foo/latest/source/\""));
            assert!(body.contains("<a href=\"/crate/foo/latest\""));
            Ok(())
        });
    }

    #[test]
    fn crate_version_not_found() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .features(HashMap::new())
                .create()?;

            let resp = env.frontend().get("/crate/foo/0.2.0/features").send()?;
            dbg!(resp.url().as_str());
            assert!(resp.url().as_str().ends_with("/crate/foo/0.2.0/features"));
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }

    #[test]
    fn invalid_semver() {
        wrapper(|env| {
            env.fake_release()
                .name("foo")
                .version("0.1.0")
                .features(HashMap::new())
                .create()?;

            let resp = env.frontend().get("/crate/foo/0,1,0/features").send()?;
            dbg!(resp.url().as_str());
            assert!(resp.url().as_str().ends_with("/crate/foo/0,1,0/features"));
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }
}
