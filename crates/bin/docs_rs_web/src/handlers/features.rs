use crate::{
    cache::CachePolicy,
    error::{AxumNope, AxumResult},
    extractors::{
        DbConnection,
        rustdoc::{PageKind, RustdocParams},
    },
    impl_axum_webpage,
    match_release::match_version,
    metadata::MetaData,
    page::templates::{RenderBrands, RenderRegular, RenderSolid, filters},
};
use anyhow::anyhow;
use askama::Template;
use axum::response::IntoResponse;
use docs_rs_headers::CanonicalUrl;
use docs_rs_types::{Feature as DbFeature, KrateName, ReqVersion};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

const DEFAULT_NAME: &str = "default";

#[derive(Debug, Clone)]
struct Feature {
    name: String,
    subfeatures: BTreeMap<String, SubFeature>,
}

impl From<DbFeature> for Feature {
    fn from(feature: DbFeature) -> Self {
        let subfeatures = feature
            .subfeatures
            .into_iter()
            .map(|name| {
                let feature = SubFeature::parse(&name);
                (name, feature)
            })
            .collect();
        Self {
            name: feature.name,
            subfeatures,
        }
    }
}

/// The sub-feature enabled by a [`Feature`]
#[derive(Debug, Clone, PartialEq)]
enum SubFeature {
    /// A normal feature, like `"feature-name"`.
    Feature(String),
    /// A dependency, like `"dep:package-name"`.
    Dependency(String),
    /// A dependency feature, like `"package-name?/feature-name"`.
    DependencyFeature {
        dependency: String,
        optional: bool,
        feature: String,
    },
}

impl SubFeature {
    fn parse(s: &str) -> Self {
        if let Some(dep) = s.strip_prefix("dep:") {
            return Self::Dependency(dep.into());
        }
        let Some((dependency, feature)) = s.split_once('/') else {
            return Self::Feature(s.into());
        };
        let (dependency, optional) = match dependency.strip_suffix('?') {
            Some(dep) => (dep, true),
            None => (dependency, false),
        };

        Self::DependencyFeature {
            dependency: dependency.into(),
            optional,
            feature: feature.into(),
        }
    }
}

#[derive(Template)]
#[template(path = "crate/features.html")]
#[derive(Debug, Clone)]
struct FeaturesPage {
    metadata: MetaData,
    dependencies: HashMap<String, ReqVersion>,
    sorted_features: Option<Vec<Feature>>,
    default_features: HashSet<String>,
    canonical_url: CanonicalUrl,
    is_latest_url: bool,
    params: RustdocParams,
}

impl FeaturesPage {
    fn is_default_feature(&self, feature: &str) -> bool {
        self.default_features.contains(feature)
    }
    fn dependency_version(&self, dependency: &str) -> &ReqVersion {
        self.dependencies
            .get(dependency)
            .unwrap_or(&ReqVersion::Latest)
    }
    fn dependency_params(&self, dependency: &str) -> Option<RustdocParams> {
        let version = self.dependency_version(dependency);
        let dependency: KrateName = dependency.parse().ok()?;

        Some(RustdocParams::new(dependency).with_req_version(version))
    }
}

impl_axum_webpage! {
    FeaturesPage,
    cache_policy = |page| {
        let name = &page.metadata.name;
        if page.is_latest_url {
            CachePolicy::ForeverInCdn(name.into())
        } else {
            CachePolicy::ForeverInCdnAndStaleInBrowser(name.into())
        }
    },
}

impl FeaturesPage {
    pub(crate) fn use_direct_platform_links(&self) -> bool {
        true
    }

    pub(crate) fn enabled_default_features_count(&self) -> usize {
        self.default_features
            .iter()
            .filter(|f| !f.starts_with("dep:") && *f != "default" && !f.contains('/'))
            .count()
    }

    pub(crate) fn features_count(&self) -> usize {
        let Some(features) = &self.sorted_features else {
            return 0;
        };
        if features.iter().any(|f| f.name == "default") {
            features.len() - 1
        } else {
            features.len()
        }
    }
}

pub(crate) async fn build_features_handler(
    params: RustdocParams,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    let matched_release = match_version(&mut conn, params.name(), params.req_version())
        .await?
        .assume_exact_name()?
        .into_canonical_req_version_or_else(|confirmed_name, version| {
            let params = params
                .clone()
                .with_name(confirmed_name)
                .with_req_version(version);
            AxumNope::Redirect(
                params.features_url(),
                CachePolicy::ForeverInCdn(confirmed_name.into()),
            )
        })?;
    let params = params.apply_matched_release(&matched_release);
    let version = matched_release.into_version();

    let metadata = MetaData::from_crate(
        &mut conn,
        params.name(),
        &version,
        Some(params.req_version().clone()),
    )
    .await?;

    let row = sqlx::query!(
        r#"
        SELECT
            releases.features as "features?: Vec<DbFeature>",
            releases.dependencies
        FROM releases
        INNER JOIN crates ON crates.id = releases.crate_id
        WHERE crates.name = $1 AND releases.version = $2"#,
        params.name() as _,
        version as _,
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow!("missing release"))?;

    let dependencies = get_dependency_versions(row.dependencies);
    let (sorted_features, default_features) = if let Some(raw_features) = row.features {
        let (sorted_features, default_features) = get_sorted_features(raw_features);
        (Some(sorted_features), default_features)
    } else {
        (None, Default::default())
    };

    Ok(FeaturesPage {
        metadata,
        dependencies,
        sorted_features,
        default_features,
        is_latest_url: params.req_version().is_latest(),
        canonical_url: CanonicalUrl::from_uri(
            params
                .clone()
                .with_req_version(ReqVersion::Latest)
                .features_url(),
        ),
        params,
    }
    .into_response())
}

/// Turns the raw JSON `dependencies` into a [`HashMap`] of dependencies and their versions.
fn get_dependency_versions(raw_dependencies: Option<Value>) -> HashMap<String, ReqVersion> {
    let mut map = HashMap::new();

    if let Some(deps) = raw_dependencies.as_ref().and_then(Value::as_array) {
        for value in deps {
            let name = value.get(0).and_then(Value::as_str);
            let version = value.get(1).and_then(Value::as_str);
            if let (Some(name), Some(version)) = (name, version) {
                let req_version = version.parse().unwrap_or(ReqVersion::Latest);
                map.insert(name.into(), req_version);
            }
        }
    }

    map
}

/// Converts raw [`DbFeature`]s into a sorted list of [`Feature`]s and a Set of default features.
///
/// The sorting order depends on depth-first traversal starting at the `"default"` feature,
/// and falls back to alphabetic sorting for all non-default features.
fn get_sorted_features(raw_features: Vec<DbFeature>) -> (Vec<Feature>, HashSet<String>) {
    let mut all_features: BTreeMap<_, _> = raw_features
        .into_iter()
        .filter(|feature| !feature.is_private())
        .map(|feature| (feature.name.clone(), Feature::from(feature)))
        .collect();

    let mut default_features = HashSet::new();
    let mut sorted_features = Vec::new();

    // this does a depth-first traversal starting at the special `"default"` feature
    if all_features.contains_key(DEFAULT_NAME) {
        let mut queue = VecDeque::new();
        queue.push_back(DEFAULT_NAME.to_owned());

        while let Some(name) = queue.pop_front() {
            if let Some(feature) = all_features.remove(&name) {
                feature
                    .subfeatures
                    .keys()
                    .for_each(|sub| queue.push_back(sub.clone()));

                sorted_features.push(feature);
            }
            default_features.insert(name);
        }
    }

    // the rest of the features not reachable from `"default"` are sorted alphabetically
    let mut remaining = Vec::from_iter(all_features.into_values());
    remaining.sort_by(|f1, f2| f1.name.cmp(&f2.name));
    sorted_features.extend(remaining);

    (sorted_features, default_features)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironmentExt as _, async_wrapper,
    };
    use kuchikiki::traits::TendrilSink;
    use reqwest::StatusCode;
    use std::str::FromStr as _;

    #[test]
    fn test_parsing_raw_features() {
        let feature = SubFeature::parse("a-feature");
        assert_eq!(feature, SubFeature::Feature("a-feature".into()));

        let feature = SubFeature::parse("dep:a-dependency");
        assert_eq!(feature, SubFeature::Dependency("a-dependency".into()));

        let feature = SubFeature::parse("a-dependency/sub-feature");
        assert_eq!(
            feature,
            SubFeature::DependencyFeature {
                dependency: "a-dependency".into(),
                optional: false,
                feature: "sub-feature".into()
            }
        );

        let feature = SubFeature::parse("a-dependency?/sub-feature");
        assert_eq!(
            feature,
            SubFeature::DependencyFeature {
                dependency: "a-dependency".into(),
                optional: true,
                feature: "sub-feature".into()
            }
        );
    }

    #[test]
    fn test_feature_map_filters_private() {
        let private1 = DbFeature::new("_private1".into(), vec!["feature1".into()]);
        let feature2 = DbFeature::new("feature2".into(), Vec::new());

        let (sorted_features, _) = get_sorted_features(vec![private1, feature2]);

        assert_eq!(sorted_features.len(), 1);
        assert_eq!(sorted_features[0].name, "feature2");
    }

    #[test]
    fn test_default_tree_structure_with_nested_default() {
        let default = DbFeature::new(DEFAULT_NAME.into(), vec!["feature1".into()]);
        let non_default = DbFeature::new("non-default".into(), Vec::new());
        let feature1 = DbFeature::new(
            "feature1".into(),
            vec!["feature2".into(), "feature3".into()],
        );
        let feature2 = DbFeature::new("feature2".into(), Vec::new());
        let feature3 = DbFeature::new("feature3".into(), Vec::new());

        let (sorted_features, default_features) =
            get_sorted_features(vec![default, non_default, feature3, feature2, feature1]);

        assert_eq!(sorted_features.len(), 5);
        assert_eq!(sorted_features[0].name, "default");
        assert_eq!(sorted_features[1].name, "feature1");
        assert_eq!(sorted_features[2].name, "feature2");
        assert_eq!(sorted_features[3].name, "feature3");
        assert_eq!(sorted_features[4].name, "non-default");

        assert!(default_features.contains("feature3"));
        assert!(!default_features.contains("non-default"));
    }

    #[test]
    fn test_default_tree_structure_without_default() {
        let feature1 = DbFeature::new(
            "feature1".into(),
            vec!["feature2".into(), "feature3".into()],
        );
        let feature2 = DbFeature::new("feature2".into(), Vec::new());
        let feature3 = DbFeature::new("feature3".into(), Vec::new());

        let (sorted_features, default_features) =
            get_sorted_features(vec![feature3, feature2, feature1]);

        assert_eq!(sorted_features.len(), 3);
        assert_eq!(sorted_features[0].name, "feature1");
        assert_eq!(sorted_features[1].name, "feature2");
        assert_eq!(sorted_features[2].name, "feature3");

        assert_eq!(default_features.len(), 0);
    }

    #[test]
    fn test_default_tree_structure_single_default() {
        let default = DbFeature::new(DEFAULT_NAME.into(), Vec::new());
        let non_default = DbFeature::new("non-default".into(), Vec::new());

        let (sorted_features, default_features) = get_sorted_features(vec![default, non_default]);

        assert_eq!(sorted_features.len(), 2);
        assert_eq!(sorted_features[0].name, "default");
        assert_eq!(sorted_features[1].name, "non-default");

        assert_eq!(default_features.len(), 1);
        assert!(default_features.contains("default"));
    }

    #[test]
    fn test_order_features_and_get_len_without_default() {
        let feature1 = DbFeature::new(
            "feature1".into(),
            vec!["feature10".into(), "feature11".into()],
        );
        let feature2 = DbFeature::new("feature2".into(), vec!["feature20".into()]);
        let feature3 = DbFeature::new("feature3".into(), Vec::new());

        let (sorted_features, default_features) =
            get_sorted_features(vec![feature3, feature2, feature1]);

        assert_eq!(sorted_features.len(), 3);
        assert_eq!(sorted_features[0].name, "feature1");
        assert_eq!(sorted_features[1].name, "feature2");
        assert_eq!(sorted_features[2].name, "feature3");

        assert_eq!(default_features.len(), 0);
    }

    #[test]
    fn semver_redirect() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.2.1")
                .features(BTreeMap::new())
                .create()
                .await?;

            let web = env.web_app().await;
            web.assert_redirect_cached(
                "/crate/foo/~0.2/features",
                "/crate/foo/0.2.1/features",
                CachePolicy::ForeverInCdn(KrateName::from_str("foo").unwrap().into()),
                env.config(),
            )
            .await?;
            Ok(())
        });
    }

    #[test]
    fn specific_version_correctly_cached() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.2.0")
                .features(BTreeMap::new())
                .create()
                .await?;

            let web = env.web_app().await;
            let resp = web.get("/crate/foo/0.2.0/features").await?;
            assert!(resp.status().is_success());
            resp.assert_cache_control(
                CachePolicy::ForeverInCdnAndStaleInBrowser(
                    KrateName::from_str("foo").unwrap().into(),
                ),
                env.config(),
            );
            Ok(())
        });
    }

    #[test]
    fn latest_200() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .features(BTreeMap::new())
                .create()
                .await?;

            env.fake_release()
                .await
                .name("foo")
                .version("0.2.0")
                .features(BTreeMap::new())
                .create()
                .await?;

            let web = env.web_app().await;
            let resp = web.get("/crate/foo/latest/features").await?;
            assert!(resp.status().is_success());
            resp.assert_cache_control(
                CachePolicy::ForeverInCdn(KrateName::from_str("foo").unwrap().into()),
                env.config(),
            );
            let body = resp.text().await?;
            assert!(body.contains("<a href=\"/crate/foo/latest/builds\""));
            assert!(body.contains("<a href=\"/crate/foo/latest/source/\""));
            assert!(body.contains("<a href=\"/crate/foo/latest\""));
            Ok(())
        });
    }

    #[test]
    fn crate_version_not_found() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .features(BTreeMap::new())
                .create()
                .await?;

            let web = env.web_app().await;
            let resp = web.get("/crate/foo/0.2.0/features").await?;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }

    #[test]
    fn invalid_semver() {
        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .features(BTreeMap::new())
                .create()
                .await?;

            let web = env.web_app().await;
            let resp = web.get("/crate/foo/0,1,0/features").await?;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            Ok(())
        });
    }

    // This test ensures that the count of feature flags is correct, in particular the count of
    // features enabled by default.
    #[test]
    fn test_features_count() {
        async_wrapper(|env| async move {
            let features = vec![
                (
                    "default".to_owned(),
                    vec![
                        "bla".to_owned(),
                        "dep:what".to_owned(),
                        "whatever/wut".to_owned(),
                    ],
                ),
                ("bla".to_owned(), vec![]),
                ("blob".to_owned(), vec![]),
            ];
            env.fake_release()
                .await
                .name("foo")
                .version("0.1.0")
                .features(features.into_iter().collect::<BTreeMap<_, _>>())
                .create()
                .await?;

            let web = env.web_app().await;

            let page = kuchikiki::parse_html()
                .one(web.get("/crate/foo/0.1.0/features").await?.text().await?);
            let text = page.select_first("#main > p").unwrap().text_contents();
            // It should only contain one feature enabled by default since the others are either
            // enabling a dependency (`dep:what`) or enabling a feature from a dependency
            // (`whatever/wut`).
            assert_eq!(
                text,
                "This version has 2 feature flags, 1 of them enabled by default."
            );

            Ok(())
        });
    }
}
