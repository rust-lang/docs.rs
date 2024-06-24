use crate::{
    db::types::Feature,
    impl_axum_webpage,
    web::{
        cache::CachePolicy,
        error::{AxumNope, AxumResult},
        extractors::{DbConnection, Path},
        headers::CanonicalUrl,
        match_version, MetaData, ReqVersion,
    },
};
use anyhow::anyhow;
use axum::response::IntoResponse;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

const DEFAULT_NAME: &str = "default";

#[derive(Debug, Clone, Serialize)]
struct DocsFeature {
    name: String,
    subfeatures: Vec<String>,
    is_default: bool,
}

type AllFeatures = HashMap<String, DocsFeature>;

#[derive(Debug, Clone, Serialize)]
struct FeaturesPage {
    metadata: MetaData,
    all_features: AllFeatures,
    sorted_features: Option<Vec<String>>,
    default_len: usize,
    canonical_url: CanonicalUrl,
    is_latest_url: bool,
    use_direct_platform_links: bool,
}

impl_axum_webpage! {
    FeaturesPage = "crate/features.html",
    cache_policy = |page| if page.is_latest_url {
        CachePolicy::ForeverInCdn
    } else {
        CachePolicy::ForeverInCdnAndStaleInBrowser
    },
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

    let mut all_features = HashMap::new();
    let mut sorted_features = None;
    let mut default_len = 0;

    if let Some(raw_features) = row.features {
        let result = order_features_and_count_default_len(raw_features);
        all_features = result.0;
        sorted_features = Some(result.1);
        default_len = result.2;
    }

    Ok(FeaturesPage {
        metadata,
        all_features,
        sorted_features,
        default_len,
        is_latest_url: req_version.is_latest(),
        canonical_url: CanonicalUrl::from_path(format!("/crate/{}/latest/features", &name)),
        use_direct_platform_links: true,
    }
    .into_response())
}

fn order_features_and_count_default_len(raw: Vec<Feature>) -> (AllFeatures, Vec<String>, usize) {
    let mut all_features = get_all_features(raw);
    let sorted_features = get_sorted_features(&mut all_features);

    let default_len = all_features.values().filter(|f| f.is_default).count();

    (all_features, sorted_features, default_len)
}

/// This flags all features as being reachable from `"default"`,
/// and returns them as a sorted list.
///
/// The sorting order depends on depth-first traversal of the default features,
/// and alphabetically otherwise.
fn get_sorted_features(all_features: &mut AllFeatures) -> Vec<String> {
    let mut sorted_features = Vec::new();
    let mut working_features: HashMap<&str, &mut DocsFeature> = all_features
        .iter_mut()
        .map(|(k, v)| (k.as_str(), v))
        .collect();

    // this does a depth-first traversal starting at the special `"default"` feature
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(DEFAULT_NAME);

    while let Some(name) = queue.pop_front() {
        if let Some(feature) = working_features.remove(name) {
            feature
                .subfeatures
                .iter()
                .for_each(|sub| queue.push_back(sub.as_str()));
            feature.is_default = true;
            sorted_features.push(feature.name.clone());
        }
    }

    // the rest of the features not reachable from `"default"` are sorted alphabetically
    let mut remaining = Vec::from_iter(working_features.into_values());
    remaining.sort_by(|f1, f2| f2.name.cmp(&f1.name));
    sorted_features.extend(remaining.into_iter().map(|f| f.name.clone()).rev());

    sorted_features
}

/// Parses the raw [`Feature`] into a map of the more structured [`DocsFeature`].
fn get_all_features(raw: Vec<Feature>) -> AllFeatures {
    raw.into_iter()
        .filter(|feature| !feature.is_private())
        .map(|feature| {
            (
                feature.name.clone(),
                DocsFeature {
                    name: feature.name,
                    subfeatures: feature.subfeatures,
                    is_default: false,
                },
            )
        })
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
        let all_features = get_all_features(raw);

        assert_eq!(all_features.len(), 1);
        assert!(all_features.contains_key(&feature2.name));
        assert!(!all_features.contains_key(&private1.name));
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
        let mut all_features = get_all_features(raw);
        let sorted_features = get_sorted_features(&mut all_features);

        assert_eq!(all_features.len(), 5);

        assert_eq!(
            sorted_features,
            vec![
                "default".to_string(),
                "feature1".into(),
                "feature2".into(),
                "feature3".into(),
                "non-default".into()
            ]
        );
        assert!(all_features["feature3"].is_default);
        assert!(!all_features["non-default"].is_default);
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
        let mut all_features = get_all_features(raw);
        let sorted_features = get_sorted_features(&mut all_features);

        assert_eq!(
            sorted_features,
            vec!["feature1".to_string(), "feature2".into(), "feature3".into()]
        );
        assert!(!all_features["feature1"].is_default);
        assert!(!all_features["feature2"].is_default);
        assert!(!all_features["feature3"].is_default);
    }

    #[test]
    fn test_default_tree_structure_single_default() {
        let default = Feature::new(DEFAULT_NAME.into(), Vec::new());
        let non_default = Feature::new("non-default".into(), Vec::new());

        let raw = vec![default.clone(), non_default.clone()];
        let mut all_features = get_all_features(raw);
        let sorted_features = get_sorted_features(&mut all_features);

        assert_eq!(
            sorted_features,
            vec!["default".to_string(), "non-default".into()]
        );
        assert!(all_features["default"].is_default);
        assert!(!all_features["non-default"].is_default);
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
        let (_all_features, sorted_features, default_len) =
            order_features_and_count_default_len(raw);

        assert_eq!(
            sorted_features,
            vec!["feature1".to_string(), "feature2".into(), "feature3".into()]
        );
        assert_eq!(default_len, 0);
    }

    #[test]
    fn test_order_features_and_get_len_single_default() {
        let default = Feature::new(DEFAULT_NAME.into(), Vec::new());
        let non_default = Feature::new("non-default".into(), Vec::new());

        let raw = vec![default.clone(), non_default.clone()];
        let (_all_features, sorted_features, default_len) =
            order_features_and_count_default_len(raw);

        assert_eq!(
            sorted_features,
            vec!["default".to_string(), "non-default".into()]
        );
        assert_eq!(default_len, 1);
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
