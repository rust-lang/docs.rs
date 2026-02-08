//! special rustdoc extractors

use crate::{error::AxumNope, extractors::Path, match_release::MatchedRelease, metadata::MetaData};
use anyhow::Result;
use axum::{
    RequestPartsExt,
    extract::{FromRequestParts, MatchedPath},
    http::{Uri, request::Parts},
};
use docs_rs_types::{BuildId, CompressionAlgorithm, KrateName, ReqVersion};
use docs_rs_uri::{EscapedURI, url_decode};
use serde::{Deserialize, Serialize};

const INDEX_HTML: &str = "index.html";
const FOLDER_AND_INDEX_HTML: &str = "/index.html";

pub(crate) const ROOT_RUSTDOC_HTML_FILES: &[&str] = &[
    "all.html",
    "help.html",
    "settings.html",
    "scrape-examples-help.html",
];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) enum PageKind {
    Rustdoc,
    Source,
}

/// Extractor for rustdoc parameters from a request.
///
/// Among other things, centralizes
/// * how we parse & interpret rustdoc related URL alements
/// * how we generate rustdoc related URLs shown in interefaces.
/// * if there is one, where to find the related file in the rustdoc build output.
///
/// All of these have more or less detail depending on how much metadata we have here.
/// Maintains some additional fields containing "fixed" things, whos quality
/// gets better the more metadata we provide.
#[derive(Clone, PartialEq, Serialize)]
pub(crate) struct RustdocParams {
    // optional behaviour marker
    page_kind: Option<PageKind>,

    original_uri: Option<EscapedURI>,
    name: KrateName,
    req_version: ReqVersion,
    doc_target: Option<String>,
    inner_path: Option<String>,
    static_route_suffix: Option<String>,

    doc_targets: Option<Vec<String>>,
    default_target: Option<String>,
    target_name: Option<String>,

    merged_inner_path: Option<String>,
}

impl std::fmt::Debug for RustdocParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RustdocParams")
            .field("page_kind", &self.page_kind)
            .field("original_uri", &self.original_uri)
            .field("name", &self.name)
            .field("req_version", &self.req_version)
            .field("doc_target", &self.doc_target)
            .field("inner_path", &self.inner_path)
            .field("doc_targets", &self.doc_targets)
            .field("default_target", &self.default_target)
            .field("target_name", &self.target_name)
            .field("static_route_suffix", &self.static_route_suffix)
            .field("merged_inner_path", &self.merged_inner_path)
            // also include some method outputs
            .field("rustdoc_url()", &self.rustdoc_url())
            .field("crate_details_url()", &self.crate_details_url())
            .field("platforms_partial_url()", &self.platforms_partial_url())
            .field("releases_partial_url()", &self.releases_partial_url())
            .field("builds_url()", &self.builds_url())
            .field("build_status_url()", &self.build_status_url())
            .field(
                "build_details_url(42, None)",
                &self.build_details_url(BuildId(42), None),
            )
            .field(
                "build_details_url(42, Some(\"log.txt\")",
                &self.build_details_url(BuildId(42), Some("log.txt")),
            )
            .field("features_url()", &self.features_url())
            .field("source_url()", &self.source_url())
            .field("target_redirect_url()", &self.target_redirect_url())
            .field("storage_path()", &self.storage_path())
            .field("generate_fallback_url()", &self.generate_fallback_url())
            .field("path_is_folder()", &self.path_is_folder())
            .field("file_extension()", &self.file_extension())
            .finish()
    }
}

/// the parameters that might come as url parameters via route.
/// All except the crate name are optional or have a default,
/// so this extractor can be used in many handlers with a variety of
/// specificity of the route.
#[derive(Debug, Deserialize)]
pub(crate) struct UrlParams {
    pub name: KrateName,
    #[serde(default)]
    pub version: ReqVersion,
    pub target: Option<String>,
    pub path: Option<String>,
}

impl<S> FromRequestParts<S> for RustdocParams
where
    S: Send + Sync,
{
    type Rejection = AxumNope;

    /// extract rustdoc parameters from request parts.
    ///
    /// For now, we're using specificially named path parameters, most are optional:
    /// * `{name}` (mandatory) => crate name
    /// * `{version}` (optional) => request version
    /// * `{target}` (optional) => doc target
    /// * `{path}` (optional) => inner path
    ///
    /// We also extract & store the original URI, and also use it to find a potential static
    /// route stuffix (e.g. the `/settings.html` in the `/{krate}/{version}/settings.html` route).
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Path(params) = parts
            .extract::<Path<UrlParams>>()
            .await
            .map_err(|err| AxumNope::BadRequest(err.into()))?;

        let original_uri = parts.extract::<Uri>().await.expect("infallible extractor");

        let matched_path = parts
            .extract::<MatchedPath>()
            .await
            .map_err(|err| AxumNope::BadRequest(err.into()))?;

        Ok(Self::from_parts(params, original_uri, matched_path)?)
    }
}

/// Builder-style methods to create & update the parameters.
#[allow(dead_code)]
impl RustdocParams {
    pub(crate) fn new(name: impl Into<KrateName>) -> Self {
        Self {
            name: name.into(),
            req_version: ReqVersion::default(),
            original_uri: None,
            doc_target: None,
            inner_path: None,
            page_kind: None,
            static_route_suffix: None,
            doc_targets: None,
            default_target: None,
            target_name: None,
            merged_inner_path: None,
        }
    }

    /// create RustdocParams with the given parts.
    ///
    /// Useful when you don't want to use struct as extractor directly,
    /// for example when you manually change things.
    pub(crate) fn from_parts(
        params: UrlParams,
        original_uri: Uri,
        matched_path: MatchedPath,
    ) -> Result<Self> {
        let static_route_suffix = {
            let uri_path = url_decode(original_uri.path())?;
            let matched_route = url_decode(matched_path.as_str())?;

            find_static_route_suffix(&matched_route, &uri_path)
        };

        Ok(RustdocParams::new(params.name)
            .with_req_version(params.version)
            .with_maybe_doc_target(params.target)
            .with_maybe_inner_path(params.path)
            .with_original_uri(original_uri)
            .with_maybe_static_route_suffix(static_route_suffix))
    }

    fn try_update<F>(self, f: F) -> Result<Self>
    where
        F: FnOnce(Self) -> Result<Self>,
    {
        let mut new = f(self)?;
        new.parse();
        Ok(new)
    }

    fn update<F>(self, f: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        self.try_update(|mut params| {
            params = f(params);
            Ok(params)
        })
        .expect("infallible")
    }

    pub(crate) fn from_metadata(metadata: &MetaData) -> Self {
        RustdocParams::new(metadata.name.clone()).apply_metadata(metadata)
    }

    pub(crate) fn apply_metadata(self, metadata: &MetaData) -> RustdocParams {
        self.with_name(metadata.name.clone())
            .with_req_version(&metadata.req_version)
            // first set the doc-target list
            .with_maybe_doc_targets(metadata.doc_targets.clone())
            // then the default target, so we can validate it.
            .with_maybe_default_target(metadata.default_target.as_deref())
            .with_maybe_target_name(metadata.target_name.as_deref())
    }

    pub(crate) fn from_matched_release(matched_release: &MatchedRelease) -> Self {
        RustdocParams::new(matched_release.name.clone()).apply_matched_release(matched_release)
    }

    pub(crate) fn apply_matched_release(self, matched_release: &MatchedRelease) -> RustdocParams {
        let release = &matched_release.release;
        self.with_name(matched_release.name.clone())
            .with_req_version(&matched_release.req_version)
            .with_maybe_doc_targets(release.doc_targets.as_deref())
            .with_maybe_default_target(release.default_target.as_deref())
            .with_maybe_target_name(release.target_name.as_deref())
    }

    pub(crate) fn name(&self) -> &KrateName {
        &self.name
    }
    pub(crate) fn with_name(self, name: impl Into<KrateName>) -> Self {
        self.update(|mut params| {
            params.name = name.into();
            params
        })
    }

    pub(crate) fn req_version(&self) -> &ReqVersion {
        &self.req_version
    }
    pub(crate) fn with_req_version(self, version: impl Into<ReqVersion>) -> Self {
        self.update(|mut params| {
            params.req_version = version.into();
            params
        })
    }
    #[cfg(test)]
    pub(crate) fn try_with_req_version<V>(self, version: V) -> Result<Self>
    where
        V: TryInto<ReqVersion>,
        V::Error: std::error::Error + Send + Sync + 'static,
    {
        use anyhow::Context as _;
        self.try_update(|mut params| {
            params.req_version = version.try_into().context("couldn't parse version")?;
            Ok(params)
        })
    }

    pub(crate) fn inner_path(&self) -> &str {
        if self.page_kind == Some(PageKind::Rustdoc)
            && let Some(merged_inner_path) = self.merged_inner_path.as_deref()
        {
            merged_inner_path
        } else {
            self.inner_path.as_deref().unwrap_or_default()
        }
    }
    pub(crate) fn with_inner_path(self, inner_path: impl Into<String>) -> Self {
        self.with_maybe_inner_path(Some(inner_path))
    }
    pub(crate) fn with_maybe_inner_path(self, inner_path: Option<impl Into<String>>) -> Self {
        self.update(|mut params| {
            params.inner_path = inner_path.map(|t| t.into().trim().to_owned());
            params
        })
    }

    pub(crate) fn original_uri(&self) -> Option<&EscapedURI> {
        self.original_uri.as_ref()
    }
    pub(crate) fn with_original_uri(self, original_uri: impl Into<EscapedURI>) -> Self {
        self.with_maybe_original_uri(Some(original_uri))
    }
    pub(crate) fn with_maybe_original_uri(
        self,
        original_uri: Option<impl Into<EscapedURI>>,
    ) -> Self {
        self.update(|mut params| {
            params.original_uri = original_uri.map(Into::into);
            params
        })
    }
    #[cfg(test)]
    pub(crate) fn try_with_original_uri<V>(self, original_uri: V) -> Result<Self>
    where
        V: TryInto<EscapedURI>,
        V::Error: std::error::Error + Send + Sync + 'static,
    {
        use anyhow::Context as _;
        self.try_update(|mut params| {
            params.original_uri = Some(original_uri.try_into().context("couldn't parse uri")?);
            Ok(params)
        })
    }
    pub(crate) fn file_extension(&self) -> Option<&str> {
        self.original_uri()
            .as_ref()
            .and_then(|uri| get_file_extension(uri.path()))
    }
    pub(crate) fn original_path(&self) -> &str {
        self.original_uri()
            .as_ref()
            .map(|p| p.path())
            .unwrap_or_default()
    }
    pub(crate) fn path_is_folder(&self) -> bool {
        path_is_folder(self.original_path())
    }

    pub(crate) fn page_kind(&self) -> Option<&PageKind> {
        self.page_kind.as_ref()
    }
    pub(crate) fn with_page_kind(self, page_kind: impl Into<PageKind>) -> Self {
        self.with_maybe_page_kind(Some(page_kind))
    }
    pub(crate) fn with_maybe_page_kind(self, page_kind: Option<impl Into<PageKind>>) -> Self {
        self.update(|mut params| {
            params.page_kind = page_kind.map(Into::into);
            params
        })
    }

    pub(crate) fn default_target(&self) -> Option<&str> {
        self.default_target.as_deref()
    }
    pub(crate) fn with_default_target(self, default_target: impl Into<String>) -> Self {
        self.with_maybe_default_target(Some(default_target))
    }
    pub(crate) fn with_maybe_default_target(
        self,
        default_target: Option<impl Into<String>>,
    ) -> Self {
        self.update(|mut params| {
            params.default_target = default_target.map(Into::into);
            params
        })
    }

    pub(crate) fn target_name(&self) -> Option<&str> {
        self.target_name.as_deref()
    }
    pub(crate) fn with_target_name(self, target_name: impl Into<String>) -> Self {
        self.with_maybe_target_name(Some(target_name))
    }
    pub(crate) fn with_maybe_target_name(self, target_name: Option<impl Into<String>>) -> Self {
        self.update(|mut params| {
            params.target_name = target_name.map(Into::into);
            params
        })
    }

    #[cfg(test)]
    pub(crate) fn with_static_route_suffix(self, static_route_suffix: impl Into<String>) -> Self {
        self.with_maybe_static_route_suffix(Some(static_route_suffix))
    }
    pub(crate) fn with_maybe_static_route_suffix(
        self,
        static_route_suffix: Option<impl Into<String>>,
    ) -> Self {
        self.update(|mut params| {
            params.static_route_suffix = static_route_suffix.map(Into::into);
            params
        })
    }

    pub(crate) fn doc_target(&self) -> Option<&str> {
        self.doc_target.as_deref()
    }
    pub(crate) fn with_doc_target(self, doc_target: impl Into<String>) -> Self {
        self.with_maybe_doc_target(Some(doc_target))
    }
    /// set the "doc taget" parameter.
    /// Might not be a target, depending on how it's generated.
    pub(crate) fn with_maybe_doc_target(self, doc_target: Option<impl Into<String>>) -> Self {
        self.update(|mut params| {
            params.doc_target = doc_target.map(Into::into);
            params
        })
    }

    pub(crate) fn doc_targets(&self) -> Option<&[String]> {
        self.doc_targets.as_deref()
    }
    pub(crate) fn with_doc_targets(
        self,
        doc_targets: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.with_maybe_doc_targets(Some(doc_targets))
    }
    pub(crate) fn with_maybe_doc_targets(
        self,
        doc_targets: Option<impl IntoIterator<Item = impl Into<String>>>,
    ) -> Self {
        self.update(|mut params| {
            params.doc_targets =
                doc_targets.map(|doc_targets| doc_targets.into_iter().map(Into::into).collect());
            params
        })
    }

    pub(crate) fn doc_target_or_default(&self) -> Option<&str> {
        self.doc_target().or(self.default_target.as_deref())
    }

    /// check if we have a target component in the path, that matches the default
    /// target. This affects the geneated storage path, since default target docs are at the root,
    /// and the other target docs are in subfolders named after the target.
    pub(crate) fn target_is_default(&self) -> bool {
        self.default_target
            .as_deref()
            .is_some_and(|t| self.doc_target() == Some(t))
    }
}

/// parser methods
impl RustdocParams {
    fn fix_target_and_path(&mut self) {
        let Some(doc_targets) = &self.doc_targets else {
            // no doc targets given, so we can't fix anything here.
            return;
        };

        let is_valid_target = |t: &str| doc_targets.iter().any(|s| s == t);

        let inner_path = self
            .inner_path
            .as_deref()
            .unwrap_or("")
            .trim_start_matches('/')
            .trim()
            .to_string();

        let (doc_target, inner_path) = if let Some(given_target) = self
            .doc_target
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if is_valid_target(given_target) {
                (Some(given_target.to_string()), inner_path)
            } else {
                // The given `doc_target` is not in the list of valid targets,
                // so we assume it's part of the path.
                let path = if inner_path.is_empty() {
                    if self.original_path().ends_with('/') {
                        format!("{}/", given_target)
                    } else {
                        given_target.to_string()
                    }
                } else {
                    format!("{}/{}", given_target, inner_path)
                };
                (None, path)
            }
        } else {
            // No `doc_target` was given, so we try to extract it from the first component of the path.
            if let Some((potential_target, rest)) = inner_path.split_once('/') {
                if is_valid_target(potential_target) {
                    (Some(potential_target.to_string()), rest.to_string())
                } else {
                    // The first path component is not a valid target.
                    (None, inner_path)
                }
            } else {
                // The path has no slashes, so the whole path could be a target.
                if is_valid_target(&inner_path) {
                    (Some(inner_path), String::new())
                } else {
                    (None, inner_path)
                }
            }
        };

        debug_assert!(
            doc_target
                .as_ref()
                .is_none_or(|t| { !t.is_empty() && !t.contains('/') && t.contains('-') }),
            "doc-target {:?} has to be non-empty, shouldn't contain slashes, but has dashes",
            doc_target
        );

        debug_assert!(!inner_path.starts_with('/')); // we should trim leading slashes

        self.inner_path = Some(inner_path);
        self.doc_target = doc_target;
    }

    /// convert the raw rustdoc parameters from the request to a "parsed" version, using additional
    /// information from release metadata.
    ///
    /// Will also validate & fix the given `doc_target` URL parameter.
    fn parse(&mut self) {
        self.fix_target_and_path();

        self.merged_inner_path = None;

        // for rustdoc pages we are merging the inner path from the URL and any potential
        // static suffix on the route. For other page kinds we do not want this.
        if self.page_kind == Some(PageKind::Rustdoc)
            && let Some(suffix) = self
                .static_route_suffix
                .as_deref()
                .filter(|s| !s.is_empty())
        {
            let mut result = self.inner_path().to_owned();
            if !result.is_empty() {
                result.push('/');
            }
            result.push_str(suffix);
            self.merged_inner_path = Some(result);
        }
    }
}

/// URL & path generation for the given params.
impl RustdocParams {
    pub(crate) fn rustdoc_url(&self) -> EscapedURI {
        generate_rustdoc_url(&self.name, &self.req_version, &self.path_for_rustdoc_url())
    }

    pub(crate) fn crate_details_url(&self) -> EscapedURI {
        EscapedURI::from_path(format!("/crate/{}/{}", self.name, self.req_version))
    }

    pub(crate) fn platforms_partial_url(&self) -> EscapedURI {
        EscapedURI::from_path(format!(
            "/crate/{}/{}/menus/platforms/{}",
            self.name,
            self.req_version,
            self.path_for_rustdoc_url_for_partials()
        ))
    }

    pub(crate) fn releases_partial_url(&self) -> EscapedURI {
        EscapedURI::from_path(format!(
            "/crate/{}/{}/menus/releases/{}",
            self.name,
            self.req_version,
            self.path_for_rustdoc_url_for_partials()
        ))
    }

    pub(crate) fn builds_url(&self) -> EscapedURI {
        EscapedURI::from_path(format!("/crate/{}/{}/builds", self.name, self.req_version))
    }

    pub(crate) fn build_status_url(&self) -> EscapedURI {
        EscapedURI::from_path(format!(
            "/crate/{}/{}/status.json",
            self.name, self.req_version
        ))
    }

    pub(crate) fn build_details_url(&self, id: BuildId, filename: Option<&str>) -> EscapedURI {
        let mut path = format!("/crate/{}/{}/builds/{}", self.name, self.req_version, id);

        if let Some(filename) = filename {
            path.push('/');
            path.push_str(filename);
        }

        EscapedURI::from_path(path)
    }

    pub(crate) fn zip_download_url(&self) -> EscapedURI {
        EscapedURI::from_path(format!(
            "/crate/{}/{}/download",
            self.name, self.req_version
        ))
    }

    pub(crate) fn json_download_url(
        &self,
        wanted_compression: Option<CompressionAlgorithm>,
        format_version: Option<&str>,
    ) -> EscapedURI {
        let mut path = format!("/crate/{}/{}", self.name, self.req_version);

        if let Some(doc_target) = self.doc_target() {
            path.push_str(&format!("/{doc_target}"));
        }

        if let Some(format_version) = format_version {
            path.push_str(&format!("/json/{format_version}"));
        } else {
            path.push_str("/json");
        }

        if let Some(wanted_compression) = wanted_compression {
            path.push_str(&format!(".{}", wanted_compression.file_extension()));
        }

        EscapedURI::from_path(path)
    }

    pub(crate) fn features_url(&self) -> EscapedURI {
        EscapedURI::from_path(format!(
            "/crate/{}/{}/features",
            self.name, self.req_version
        ))
    }

    pub(crate) fn source_url(&self) -> EscapedURI {
        // if the params were created for a rustdoc page,
        // the inner path is a source file path, so is not usable for
        // source urls.
        let inner_path = if self.page_kind == Some(PageKind::Source) {
            self.inner_path()
        } else {
            ""
        };
        EscapedURI::from_path(format!(
            "/crate/{}/{}/source/{}",
            &self.name, &self.req_version, &inner_path
        ))
    }

    pub(crate) fn target_redirect_url(&self) -> EscapedURI {
        EscapedURI::from_path(format!(
            "/crate/{}/{}/target-redirect/{}",
            self.name,
            self.req_version,
            &self.path_for_rustdoc_url(),
        ))
    }

    /// generate a potential storage path where to find the file that is described by these params.
    ///
    /// This is the path _inside_ the rustdoc archive zip file we create in the build process.
    pub(crate) fn storage_path(&self) -> String {
        let mut storage_path = self.path_for_rustdoc_url();

        if path_is_folder(&storage_path) {
            storage_path.push_str(INDEX_HTML);
        }

        storage_path
    }

    fn path_for_rustdoc_url_for_partials(&self) -> String {
        if self.page_kind() == Some(&PageKind::Rustdoc) {
            generate_rustdoc_path_for_url(None, None, self.doc_target(), Some(self.inner_path()))
        } else {
            generate_rustdoc_path_for_url(None, None, self.doc_target(), None)
        }
    }

    fn path_for_rustdoc_url(&self) -> String {
        if self.page_kind() == Some(&PageKind::Rustdoc) {
            generate_rustdoc_path_for_url(
                self.target_name.as_deref(),
                self.default_target.as_deref(),
                self.doc_target(),
                Some(self.inner_path()),
            )
        } else {
            generate_rustdoc_path_for_url(
                self.target_name.as_deref(),
                self.default_target.as_deref(),
                self.doc_target(),
                None,
            )
        }
    }

    /// Generate a possible target path to redirect to, with the information we have.
    ///
    /// Built for the target-redirect view, when we don't find the
    /// target in our storage.
    ///
    /// Input is our set or parameters, plus some details from the metadata.
    ///
    /// This method is typically only used when we already know the target file doesn't exist,
    /// and we just need to redirect to a search or something similar.
    fn generate_fallback_search(&self) -> Option<String> {
        // we already split out the potentially leading target information in `Self::parse`.
        // So we have an optional target, and then the path.
        let components: Vec<_> = self
            .inner_path()
            .trim_start_matches('/')
            .split('/')
            .collect();

        let is_source_view = components.first() == Some(&"src");

        components
            .last()
            .and_then(|&last_component| {
                if last_component.is_empty() || last_component == INDEX_HTML {
                    // this is a module, we extract the module name
                    //
                    // path might look like:
                    // `/[krate]/[version]/{target_name}/{module}/index.html` (last_component is index)
                    // or
                    // `/[krate]/[version]/{target_name}/{module}/` (last_component is empty)
                    //
                    // for the search we want to use the module name.
                    components.iter().rev().nth(1).cloned()
                } else if !is_source_view {
                    // this is an item, typically the filename (last component) is something
                    // `trait.SomeAwesomeStruct.html`, where we want `SomeAwesomeStruct` for
                    // the search
                    last_component.split('.').nth(1)
                } else {
                    // this is from the rustdoc source view.
                    // Example last component:
                    // `tuple_impl.rs.html` where we want just `tuple_impl` for the search.
                    last_component.strip_suffix(".rs.html")
                }
            })
            .map(ToString::to_string)
    }

    pub(crate) fn generate_fallback_url(&self) -> EscapedURI {
        let rustdoc_url = self.clone().with_inner_path("").rustdoc_url();

        if let Some(search_item) = self.generate_fallback_search() {
            rustdoc_url.append_query_pair("search", search_item)
        } else {
            rustdoc_url
        }
    }
}

fn get_file_extension(path: &str) -> Option<&str> {
    path.rsplit_once('.').and_then(|(_, ext)| {
        if ext.contains('/') {
            // to handle cases like `foo.html/bar` where I want `None`
            None
        } else {
            Some(ext)
        }
    })
}

fn generate_rustdoc_url(name: &KrateName, version: &ReqVersion, path: &str) -> EscapedURI {
    EscapedURI::from_path(format!("/{}/{}/{}", name, version, path))
}

fn generate_rustdoc_path_for_url(
    target_name: Option<&str>,
    default_target: Option<&str>,
    mut doc_target: Option<&str>,
    mut inner_path: Option<&str>,
) -> String {
    // if we have an "unparsed" set of params, we might have a part of
    // the inner path in `doc_target`. Thing is:
    // We don't know if that's a real target, or a part of the path,
    // But the "saner" default for this method is to treat it as part
    // of the path, not a potential doc target.
    let inner_path = if target_name.is_none()
        && default_target.is_none()
        && let (Some(doc_target), Some(inner_path)) = (doc_target.take(), inner_path.as_mut())
        && !doc_target.is_empty()
    {
        Some(format!("{doc_target}/{inner_path}"))
    } else {
        inner_path.map(|s| s.to_string())
    };

    // first validate & fix the inner path to use.
    let result = if let Some(path) = inner_path
        && !path.is_empty()
        && path != INDEX_HTML
    {
        // for none-elements paths we have to guarantee that we have a
        // trailing slash, otherwise the rustdoc-url won't hit the html-handler and
        // lead to redirect loops.
        if path.contains('/') {
            // just use the given inner to start, if:
            // * it's not empty
            // * it's not just "index.html"
            // * we have a slash in the path.
            path.to_string()
        } else if ROOT_RUSTDOC_HTML_FILES.contains(&path.as_str()) {
            // special case: some files are at the root of the rustdoc output,
            // without a trailing slash, and the routes are fine with that.
            // e.g. `/help.html`, `/settings.html`, `/all.html`, ...
            path.to_string()
        } else if let Some(target_name) = target_name {
            if target_name == path {
                // when we have the target name as path, without a trailing slash,
                // just add the slash.
                format!("{}/", path)
            } else {
                // when someone just attaches some path to the URL, like
                // `/{krate}/{version}/somefile.html`, we assume they meant
                // `/{krate}/{version}/{target_name}/somefile.html`.
                format!("{}/{}", target_name, path)
            }
        } else {
            // fallback: just attach a slash and redirect.
            format!("{}/", path)
        }
    } else if let Some(target_name) = target_name {
        // after having no usable given path, we generate one with the
        // target name, if we have one/.
        format!("{}/", target_name)
    } else {
        // no usable given path:
        // * empty
        // * "index.html"
        String::new()
    };

    // then prepent the inner path with the doc target, if it's not the default target.
    let result = match (doc_target, default_target) {
        // add  a subfolder for any non-default target.
        (Some(doc_target), Some(default_target)) if doc_target != default_target => {
            format!("{}/{}", doc_target, result)
        }
        // when we don't know which the default target is, always add the target,
        // and assume it's non-default.
        (Some(doc_target), None) => {
            format!("{}/{}", doc_target, result)
        }

        // other cases: don't do anything, keep the last result:
        // * no doc_target, has default target -> no target in url
        // * no doc_target, no default target -> no target in url
        _ => result,
    };

    // case handled above and replaced with an empty path
    debug_assert_ne!(result, INDEX_HTML);

    // for folders we might have `/index.html` at the end.
    // We want to normalize the requests for folders, so a trailing `/index.html`
    // will be cut off.
    if result.ends_with(FOLDER_AND_INDEX_HTML) {
        result.trim_end_matches(INDEX_HTML).to_string()
    } else {
        result
    }
}

fn path_is_folder(path: impl AsRef<str>) -> bool {
    let path = path.as_ref();
    path.is_empty() || path.ends_with('/')
}

/// we sometimes have routes with a static suffix.
///
/// For example: `/{name}/{version}/help.html`
/// In this case, we won't get the `help.html` part in our `path` parameter, since there is
/// no `{*path}` in the route.
///
/// We're working around that by re-attaching the static suffix. This function is to find the
/// shared suffix between the route and the actual path.
fn find_static_route_suffix<'a, 'b>(route: &'a str, path: &'b str) -> Option<String> {
    let mut suffix: Vec<&'a str> = Vec::new();

    for (route_component, path_component) in route.rsplit('/').zip(path.rsplit('/')) {
        if route_component.starts_with('{') && route_component.ends_with('}') {
            // we've reached a dynamic component in the route, stop here
            break;
        }

        if route_component != path_component {
            // components don't match, no static suffix.
            // Everything has to match up to the last dynamic component.
            return None;
        }

        // components match, continue to the next component
        suffix.push(route_component);
    }

    if suffix.is_empty() {
        None
    } else if let &[suffix] = suffix.as_slice()
        && suffix.is_empty()
    {
        // special case: if the suffix is just empty, return None
        None
    } else {
        Some(suffix.iter().rev().fold(String::new(), |mut acc, s| {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(s);
            acc
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{AxumResponseTestExt, AxumRouterTestExt};
    use axum::{Router, routing::get};
    use docs_rs_types::{Version, testing::V1};
    use test_case::test_case;

    const KRATE: KrateName = KrateName::from_static("krate");
    const DUMMY: KrateName = KrateName::from_static("dummy");
    const CLAP: KrateName = KrateName::from_static("clap");
    const VERSION: Version = Version::new(0, 1, 0);
    static DEFAULT_TARGET: &str = "x86_64-unknown-linux-gnu";
    static OTHER_TARGET: &str = "x86_64-pc-windows-msvc";
    static UNKNOWN_TARGET: &str = "some-unknown-target";
    static TARGETS: &[&str] = &[DEFAULT_TARGET, OTHER_TARGET];

    #[test_case(
        "/{name}/{version}/help/some.html",
        "/foo/1.2.3/help/some.html"
        => Some("help/some.html".into());
        "suffix with path"
    )]
    #[test_case("/{name}/{version}/help.html", "/foo/1.2.3/help.html" => Some("help.html".into()); "simple suffix")]
    #[test_case("help.html", "help.html" => Some("help.html".into()); "simple suffix without other components")]
    #[test_case("/{name}/{version}/help/", "/foo/1.2.3/help/" => Some("help/".into()); "suffix is folder")]
    #[test_case("{name}/{version}/help/", "foo/1.2.3/help/" => Some("help/".into()); "without leading slash")]
    #[test_case("/{name}/{version}/{*path}", "/foo/1.2.3/help.html" => None; "no suffix in route")]
    #[test_case("/{name}/{version}/help.html", "/foo/1.2.3/other.html" => None; "different suffix")]
    #[test_case(
        "/{name}/{version}/some/help.html",
        "/foo/1.2.3/other/help.html"
        => None;
        "different suffix later"
    )]
    #[test_case("", "" => None; "empty strings")]
    #[test_case("/", "" => None; "one slash, one empty")]
    fn test_find_static_route_suffix(route: &str, path: &str) -> Option<String> {
        find_static_route_suffix(route, path)
    }

    #[test_case(
        "/{name}",
        RustdocParams::new(KRATE)
            .try_with_original_uri("/krate").unwrap();
        "just name"
    )]
    #[test_case(
        "/{name}/",
        RustdocParams::new(KRATE)
            .try_with_original_uri("/krate/").unwrap();
        "just name with trailing slash"
    )]
    #[test_case(
        "/{name}/{version}",
        RustdocParams::new(KRATE)
            .try_with_original_uri("/krate/latest").unwrap();
        "just name and version"
    )]
    #[test_case(
        "/{name}/{version}/{*path}",
        RustdocParams::new(KRATE)
            .try_with_original_uri("/krate/latest/static.html").unwrap()
            .with_inner_path("static.html");
        "name, version, path extract"
    )]
    #[test_case(
        "/{name}/{version}/{path}/static.html",
        RustdocParams::new(KRATE)
            .try_with_original_uri("/krate/latest/path_add/static.html").unwrap()
            .with_inner_path("path_add")
            .with_static_route_suffix("static.html");
        "name, version, path extract, static suffix"
    )]
    #[test_case(
        "/{name}/{version}/clapproc%20%60macro.html",
        RustdocParams::new(CLAP)
            .try_with_original_uri("/clap/latest/clapproc%20%60macro.html").unwrap()
            .with_static_route_suffix("clapproc `macro.html");
        "name, version, static suffix with some urlencoding"
    )]
    #[test_case(
        "/{name}/{version}/static.html",
        RustdocParams::new(KRATE)
            .try_with_original_uri("/krate/latest/static.html").unwrap()
            .with_static_route_suffix("static.html");
        "name, version, static suffix"
    )]
    #[test_case(
        "/{name}/{version}/{target}",
        RustdocParams::new(KRATE)
            .try_with_req_version("1.2.3").unwrap()
            .try_with_original_uri(format!("/krate/1.2.3/{OTHER_TARGET}")).unwrap()
            .with_doc_target(OTHER_TARGET);
        "name, version, target"
    )]
    #[test_case(
        "/{name}/{version}/{target}/folder/something.html",
        RustdocParams::new(KRATE)
            .try_with_req_version("1.2.3").unwrap()
            .try_with_original_uri(format!("/krate/1.2.3/{OTHER_TARGET}/folder/something.html")).unwrap()
            .with_doc_target(OTHER_TARGET)
            .with_static_route_suffix("folder/something.html");
        "name, version, target, static suffix"
    )]
    #[test_case(
        "/{name}/{version}/{target}/",
        RustdocParams::new(KRATE)
            .try_with_req_version("1.2.3").unwrap()
            .try_with_original_uri(format!("/krate/1.2.3/{OTHER_TARGET}/")).unwrap()
            .with_doc_target(OTHER_TARGET);
        "name, version, target trailing slash"
    )]
    #[test_case(
        "/{name}/{version}/{target}/{*path}",
        RustdocParams::new(KRATE)
            .try_with_req_version("1.2.3").unwrap()
            .try_with_original_uri(format!("/krate/1.2.3/{OTHER_TARGET}/some/path/to/a/file.html")).unwrap()
            .with_doc_target(OTHER_TARGET)
            .with_inner_path("some/path/to/a/file.html");
        "name, version, target, path"
    )]
    #[test_case(
        "/{name}/{version}/{target}/{path}/path/to/a/file.html",
        RustdocParams::new(KRATE)
            .try_with_req_version("1.2.3").unwrap()
            .try_with_original_uri(format!("/krate/1.2.3/{OTHER_TARGET}/path_add/path/to/a/file.html")).unwrap()
            .with_doc_target(OTHER_TARGET)
            .with_inner_path("path_add")
            .with_static_route_suffix("path/to/a/file.html");
        "name, version, target, path, static suffix"
    )]
    #[tokio::test]
    async fn test_extract_rustdoc_params_from_request(
        route: &str,
        expected: RustdocParams,
    ) -> anyhow::Result<()> {
        let expected = expected.with_page_kind(PageKind::Rustdoc);

        let app = Router::new().route(
            route,
            get(|params: RustdocParams| async move {
                format!("{:?}", params.with_page_kind(PageKind::Rustdoc))
            }),
        );

        let path = expected.original_uri.as_ref().unwrap().path().to_owned();

        let res = app.get(&path).await?;
        assert!(res.status().is_success());
        assert_eq!(res.text().await?, format!("{:?}", expected));

        Ok(())
    }

    #[test_case(
        None, None, false,
        None, "", "krate/index.html";
        "super empty 1"
    )]
    #[test_case(
        Some(""), Some(""), false,
        None, "", "krate/index.html";
        "super empty 2"
    )]
    // test cases when no separate "target" component was present in the params
    #[test_case(
        None, Some("/"), true,
        None, "", "krate/index.html";
        "just slash"
    )]
    #[test_case(
        None, Some("something"), false,
        None, "something", "krate/something";
        "without trailing slash"
    )]
    #[test_case(
        None, Some("settings.html"), false,
        None, "settings.html", "settings.html";
        "without trailing slash, but known root name"
    )]
    #[test_case(
        None, Some("/something"), false,
        None, "something", "krate/something";
        "leading slash is cut"
    )]
    #[test_case(
        None, Some("something/"), true,
        None, "something/", "something/index.html";
        "with trailing slash"
    )]
    // a target is given, but as first component of the path, for routes without separate
    // "target" component
    #[test_case(
        None, Some(DEFAULT_TARGET), false,
        Some(DEFAULT_TARGET), "", "krate/index.html";
        "just target without trailing slash"
    )]
    #[test_case(
        None, Some(&format!("{DEFAULT_TARGET}/")), true,
        Some(DEFAULT_TARGET), "", "krate/index.html";
        "just default target with trailing slash"
    )]
    #[test_case(
        None, Some(&format!("{DEFAULT_TARGET}/one")), false,
        Some(DEFAULT_TARGET), "one", "krate/one";
        "target + one without trailing slash"
    )]
    #[test_case(
        None, Some(&format!("{DEFAULT_TARGET}/one/")), true,
        Some(DEFAULT_TARGET), "one/", "one/index.html";
        "target + one target with trailing slash"
    )]
    #[test_case(
        None, Some(&format!("{UNKNOWN_TARGET}/one/")), true,
        None, &format!("{UNKNOWN_TARGET}/one/"), &format!("{UNKNOWN_TARGET}/one/index.html");
        "unknown target stays in path"
    )]
    #[test_case(
        None, Some(&format!("{DEFAULT_TARGET}/some/inner/path")), false,
        Some(DEFAULT_TARGET), "some/inner/path", "some/inner/path";
        "all without trailing slash"
    )]
    #[test_case(
        None, Some(&format!("{DEFAULT_TARGET}/some/inner/path/")), true,
        Some(DEFAULT_TARGET), "some/inner/path/", "some/inner/path/index.html";
        "all with trailing slash"
    )]
    // here we have a separate target path parameter, we check it and use it accordingly
    #[test_case(
        Some(DEFAULT_TARGET), None, false,
        Some(DEFAULT_TARGET), "", "krate/index.html";
        "actual target, that is default"
    )]
    #[test_case(
        Some(DEFAULT_TARGET), Some("inner/path.html"), false,
        Some(DEFAULT_TARGET), "inner/path.html", "inner/path.html";
        "actual target with path"
    )]
    #[test_case(
        Some(DEFAULT_TARGET), Some("inner/path/"), true,
        Some(DEFAULT_TARGET), "inner/path/", "inner/path/index.html";
        "actual target with path slash"
    )]
    #[test_case(
        Some(UNKNOWN_TARGET), None, true,
        None, &format!("{UNKNOWN_TARGET}/"), &format!("{UNKNOWN_TARGET}/index.html");
        "unknown target"
    )]
    #[test_case(
        Some(UNKNOWN_TARGET), None, false,
        None, UNKNOWN_TARGET, &format!("krate/{UNKNOWN_TARGET}");
        "unknown target without trailing slash"
    )]
    #[test_case(
        Some(UNKNOWN_TARGET), Some("inner/path.html"), false,
        None, &format!("{UNKNOWN_TARGET}/inner/path.html"), &format!("{UNKNOWN_TARGET}/inner/path.html");
        "unknown target with path"
    )]
    #[test_case(
        Some(OTHER_TARGET), Some("inner/path.html"), false,
        Some(OTHER_TARGET), "inner/path.html", &format!("{OTHER_TARGET}/inner/path.html");
        "other target with path"
    )]
    #[test_case(
        Some(UNKNOWN_TARGET), Some("inner/path/"), true,
        None, &format!("{UNKNOWN_TARGET}/inner/path/"), &format!("{UNKNOWN_TARGET}/inner/path/index.html");
        "unknown target with path slash"
    )]
    #[test_case(
        Some(OTHER_TARGET), Some("inner/path/"), true,
        Some(OTHER_TARGET), "inner/path/", &format!("{OTHER_TARGET}/inner/path/index.html");
        "other target with path slash"
    )]
    #[test_case(
        Some(DEFAULT_TARGET), None, false,
        Some(DEFAULT_TARGET), "", "krate/index.html";
        "pure default target, without trailing slash"
    )]
    fn test_parse(
        target: Option<&str>,
        path: Option<&str>,
        had_trailing_slash: bool,
        expected_target: Option<&str>,
        expected_path: &str,
        expected_storage_path: &str,
    ) {
        let mut dummy_path = match (target, path) {
            (Some(target), Some(path)) => format!("{}/{}", target, path),
            (Some(target), None) => target.to_string(),
            (None, Some(path)) => path.to_string(),
            (None, None) => String::new(),
        };
        dummy_path.insert(0, '/');
        if had_trailing_slash && !dummy_path.is_empty() {
            dummy_path.push('/');
        }

        let parsed = RustdocParams::new(KRATE)
            .with_page_kind(PageKind::Rustdoc)
            .with_req_version(ReqVersion::Latest)
            .with_maybe_doc_target(target)
            .with_maybe_inner_path(path)
            .try_with_original_uri(&dummy_path[..])
            .unwrap()
            .with_default_target(DEFAULT_TARGET)
            .with_target_name(KRATE.to_string())
            .with_doc_targets(TARGETS.iter().cloned());

        assert_eq!(parsed.name(), &KRATE);
        assert_eq!(parsed.req_version(), &ReqVersion::Latest);
        assert_eq!(parsed.doc_target(), expected_target);
        assert_eq!(parsed.inner_path(), expected_path);
        assert_eq!(parsed.storage_path(), expected_storage_path);
        assert_eq!(
            parsed.path_is_folder(),
            had_trailing_slash || dummy_path.ends_with('/') || dummy_path.is_empty()
        );
    }

    #[test_case("dummy/struct.WindowsOnly.html", Some("WindowsOnly"))]
    #[test_case("dummy/some_module/struct.SomeItem.html", Some("SomeItem"))]
    #[test_case("dummy/some_module/index.html", Some("some_module"))]
    #[test_case("dummy/some_module/", Some("some_module"))]
    #[test_case("src/folder1/folder2/logic.rs.html", Some("logic"))]
    #[test_case("src/non_source_file.rs", None)]
    #[test_case("html", None; "plain file without extension")]
    #[test_case("something.html", Some("html"); "plain file")]
    #[test_case("", None)]
    fn test_generate_fallback_search(path: &str, search: Option<&str>) {
        let mut params = RustdocParams::new(DUMMY)
            .try_with_req_version("0.4.0")
            .unwrap()
            // non-default target, target stays in the url
            .with_doc_target(OTHER_TARGET)
            .with_inner_path(path)
            .with_default_target(DEFAULT_TARGET)
            .with_target_name("dummy")
            .with_doc_targets(TARGETS.iter().cloned());

        assert_eq!(params.generate_fallback_search().as_deref(), search);
        assert_eq!(
            params.generate_fallback_url().to_string(),
            format!(
                "/dummy/0.4.0/x86_64-pc-windows-msvc/dummy/{}",
                search.map(|s| format!("?search={}", s)).unwrap_or_default()
            )
        );

        // change to default target, check url again
        params = params.with_doc_target(DEFAULT_TARGET);

        assert_eq!(params.generate_fallback_search().as_deref(), search);
        assert_eq!(
            params.generate_fallback_url().to_string(),
            format!(
                "/dummy/0.4.0/dummy/{}",
                search.map(|s| format!("?search={}", s)).unwrap_or_default()
            )
        );
    }

    #[test]
    fn test_parse_source() {
        let params = RustdocParams::new(DUMMY)
            .try_with_req_version("0.4.0")
            .unwrap()
            .with_inner_path("README.md")
            .with_page_kind(PageKind::Source)
            .try_with_original_uri("/crate/dummy/0.4.0/source/README.md")
            .unwrap()
            .with_default_target(DEFAULT_TARGET)
            .with_target_name("dummy")
            .with_doc_targets(TARGETS.iter().cloned());

        assert_eq!(params.rustdoc_url().to_string(), "/dummy/0.4.0/dummy/");
        assert_eq!(
            params.source_url().to_string(),
            "/crate/dummy/0.4.0/source/README.md"
        );
        assert_eq!(
            params.target_redirect_url().to_string(),
            "/crate/dummy/0.4.0/target-redirect/dummy/"
        );
    }

    #[test_case(
        None, None, None, None => ""
    )]
    #[test_case(
        Some("target_name"), None, None, None => "target_name/"
    )]
    #[test_case(
        None, None, None, Some("path/index.html") => "path/";
        "cuts trailing /index.html"
    )]
    #[test_case(
        Some("target_name"), None,
        Some(DEFAULT_TARGET), Some("inner/path.html")
        => "x86_64-unknown-linux-gnu/inner/path.html";
        "default target, but we don't know about it, keeps target"
    )]
    #[test_case(
        Some("target_name"), None,
        Some(DEFAULT_TARGET), None
        => "x86_64-unknown-linux-gnu/target_name/";
        "default target, we don't know about it, without path"
    )]
    #[test_case(
        Some("target_name"), Some(DEFAULT_TARGET),
        Some(DEFAULT_TARGET), None
        => "target_name/";
        "default-target, without path, target_name is used to generate the inner path"
    )]
    #[test_case(
        Some("target_name"), Some(DEFAULT_TARGET),
        Some(DEFAULT_TARGET), Some("inner/path.html")
        => "inner/path.html";
        "default target, with path, target_name is ignored"
    )]
    #[test_case(
        None, Some(DEFAULT_TARGET),
        Some(DEFAULT_TARGET), Some("inner/path/index.html")
        => "inner/path/";
        "default target, with path as folder with index.html"
    )]
    #[test_case(
        None, Some(DEFAULT_TARGET),
        Some(DEFAULT_TARGET), Some("inner/path/")
        => "inner/path/";
        "default target, with path as folder"
    )]
    #[test_case(
        Some("target_name"), Some(DEFAULT_TARGET),
        Some(OTHER_TARGET), None
        => "x86_64-pc-windows-msvc/target_name/";
        "non-default-target, without path, target_name is used to generate the inner path"
    )]
    #[test_case(
        Some("target_name"), Some(DEFAULT_TARGET),
        Some(OTHER_TARGET), Some("inner/path.html")
        => "x86_64-pc-windows-msvc/inner/path.html";
        "non-default target, with path, target_name is ignored"
    )]
    fn test_generate_rustdoc_path_for_url(
        target_name: Option<&str>,
        default_target: Option<&str>,
        doc_target: Option<&str>,
        inner_path: Option<&str>,
    ) -> String {
        generate_rustdoc_path_for_url(target_name, default_target, doc_target, inner_path)
    }

    #[test]
    fn test_case_1() {
        let params = RustdocParams::new(DUMMY)
            .try_with_req_version("0.2.0")
            .unwrap()
            .with_doc_target("dummy")
            .with_inner_path("struct.Dummy.html")
            .with_page_kind(PageKind::Rustdoc)
            .try_with_original_uri("/dummy/0.2.0/dummy/struct.Dummy.html")
            .unwrap()
            .with_default_target(DEFAULT_TARGET)
            .with_target_name("dummy")
            .with_doc_targets(TARGETS.iter().cloned());

        dbg!(&params);

        assert!(params.doc_target().is_none());
        assert_eq!(params.inner_path(), "dummy/struct.Dummy.html");
        assert_eq!(params.storage_path(), "dummy/struct.Dummy.html");

        let params = params.with_doc_target(DEFAULT_TARGET);
        dbg!(&params);
        assert_eq!(params.doc_target(), Some(DEFAULT_TARGET));
        assert_eq!(params.inner_path(), "dummy/struct.Dummy.html");
        assert_eq!(params.storage_path(), "dummy/struct.Dummy.html");

        let params = params.with_doc_target(OTHER_TARGET);
        assert_eq!(params.doc_target(), Some(OTHER_TARGET));
        assert_eq!(
            params.storage_path(),
            format!("{OTHER_TARGET}/dummy/struct.Dummy.html")
        );
        assert_eq!(
            params.storage_path(),
            format!("{OTHER_TARGET}/dummy/struct.Dummy.html")
        );
    }

    #[test_case(
        "/",
        None, None,
        None, ""
        ; "no target, no path"
    )]
    #[test_case(
        &format!("/{DEFAULT_TARGET}"),
        Some(DEFAULT_TARGET), None,
        Some(DEFAULT_TARGET), "";
        "existing target, no path"
    )]
    #[test_case(
        &format!("/{UNKNOWN_TARGET}"),
        Some(UNKNOWN_TARGET), None,
        None, UNKNOWN_TARGET;
        "unknown target, no path"
    )]
    #[test_case(
        &format!("/{UNKNOWN_TARGET}/"),
        Some(UNKNOWN_TARGET), Some("something/file.html"),
        None, &format!("{UNKNOWN_TARGET}/something/file.html");
        "unknown target, with path, trailling slash is kept"
    )]
    #[test_case(
        &format!("/{UNKNOWN_TARGET}/"),
        Some(UNKNOWN_TARGET), None,
        None, &format!("{UNKNOWN_TARGET}/");
        "unknown target, no path, trailling slash is kept"
    )]
    fn test_with_fixed_target_and_path(
        original_uri: &str,
        target: Option<&str>,
        path: Option<&str>,
        expected_target: Option<&str>,
        expected_path: &str,
    ) {
        let params = RustdocParams::new(KRATE)
            .try_with_req_version("0.4.0")
            .unwrap()
            .try_with_original_uri(original_uri)
            .unwrap()
            .with_maybe_doc_target(target)
            .with_maybe_inner_path(path)
            .with_doc_targets(TARGETS.iter().cloned());

        dbg!(&params);

        assert_eq!(params.doc_target(), expected_target);
        assert_eq!(params.inner_path(), expected_path);
    }

    #[test_case(
        None, None,
        None, None
        => "";
        "empty"
    )]
    #[test_case(
        None, None,
        None, Some("folder/index.html")
        => "folder/";
        "just folder index.html will be removed"
    )]
    #[test_case(
        None, None,
        None, Some(INDEX_HTML)
        => "";
        "just root index.html will be removed"
    )]
    #[test_case(
        None, Some(DEFAULT_TARGET),
        Some(DEFAULT_TARGET), None
        => "";
        "just default target"
    )]
    #[test_case(
        None, Some(DEFAULT_TARGET),
        Some(OTHER_TARGET), None
        => format!("{OTHER_TARGET}/");
        "just other target"
    )]
    #[test_case(
        Some(&KRATE), Some(DEFAULT_TARGET),
        Some(DEFAULT_TARGET), None
        => format!("{KRATE}/");
        "full with default target, target name is used"
    )]
    #[test_case(
        Some(&KRATE), Some(DEFAULT_TARGET),
        Some(OTHER_TARGET), None
        => format!("{OTHER_TARGET}/{KRATE}/");
        "full with other target, target name is used"
    )]
    #[test_case(
        Some(&KRATE), Some(DEFAULT_TARGET),
        Some(DEFAULT_TARGET), Some("inner/something.html")
        => "inner/something.html";
        "full with default target, target name is ignored"
    )]
    #[test_case(
        Some(&KRATE), Some(DEFAULT_TARGET),
        Some(OTHER_TARGET), Some("inner/something.html")
        => format!("{OTHER_TARGET}/inner/something.html");
        "full with other target, target name is ignored"
    )]
    fn test_rustdoc_path_for_url(
        target_name: Option<&KrateName>,
        default_target: Option<&str>,
        doc_target: Option<&str>,
        inner_path: Option<&str>,
    ) -> String {
        generate_rustdoc_path_for_url(
            target_name.map(|n| n.to_string()).as_deref(),
            default_target,
            doc_target,
            inner_path,
        )
    }

    #[test]
    fn test_override_page_kind() {
        let params = RustdocParams::new(KRATE)
            .try_with_original_uri("/krate/latest/path_add/static.html")
            .unwrap()
            .with_inner_path("path_add")
            .with_static_route_suffix("static.html")
            .with_default_target(DEFAULT_TARGET)
            .with_target_name(KRATE.to_string())
            .with_doc_targets(TARGETS.iter().cloned());

        // without page kind, rustdoc path doesn' thave a path, and static suffix ignored
        assert_eq!(params.rustdoc_url(), "/krate/latest/krate/");
        assert_eq!(params.source_url(), "/crate/krate/latest/source/");
        assert_eq!(
            params.target_redirect_url(),
            "/crate/krate/latest/target-redirect/krate/"
        );

        let params = params.with_page_kind(PageKind::Rustdoc);
        assert_eq!(params.rustdoc_url(), "/krate/latest/path_add/static.html");
        assert_eq!(params.source_url(), "/crate/krate/latest/source/");
        assert_eq!(
            params.target_redirect_url(),
            "/crate/krate/latest/target-redirect/path_add/static.html"
        );

        let params = params.with_page_kind(PageKind::Source);
        assert_eq!(params.rustdoc_url(), "/krate/latest/krate/");
        // just path added, not static suffix
        assert_eq!(params.source_url(), "/crate/krate/latest/source/path_add");
        assert_eq!(
            params.target_redirect_url(),
            "/crate/krate/latest/target-redirect/krate/"
        );
    }

    #[test]
    fn test_override_page_kind_with_target() {
        let params = RustdocParams::new(KRATE)
            .try_with_original_uri(format!("/krate/latest/{OTHER_TARGET}/path_add/static.html"))
            .unwrap()
            .with_inner_path("path_add")
            .with_static_route_suffix("static.html")
            .with_doc_target(OTHER_TARGET)
            .with_default_target(DEFAULT_TARGET)
            .with_target_name(KRATE.to_string())
            .with_doc_targets(TARGETS.iter().cloned());

        // without page kind, rustdoc path doesn' thave a path, and static suffix ignored
        assert_eq!(
            params.rustdoc_url(),
            format!("/krate/latest/{OTHER_TARGET}/krate/")
        );
        assert_eq!(params.source_url(), "/crate/krate/latest/source/");
        assert_eq!(
            params.target_redirect_url(),
            format!("/crate/krate/latest/target-redirect/{OTHER_TARGET}/krate/")
        );

        // same when the pagekind is "Source"
        let params = params.with_page_kind(PageKind::Source);
        assert_eq!(
            params.rustdoc_url(),
            format!("/krate/latest/{OTHER_TARGET}/krate/")
        );
        assert_eq!(params.source_url(), "/crate/krate/latest/source/path_add");
        assert_eq!(
            params.target_redirect_url(),
            format!("/crate/krate/latest/target-redirect/{OTHER_TARGET}/krate/")
        );

        // with page-kind "Rustdoc", we get the full path with static suffix
        let params = params.with_page_kind(PageKind::Rustdoc);
        dbg!(&params);
        assert_eq!(
            params.rustdoc_url(),
            format!("/krate/latest/{OTHER_TARGET}/path_add/static.html")
        );
        assert_eq!(params.source_url(), format!("/crate/krate/latest/source/"));
        assert_eq!(
            params.target_redirect_url(),
            format!("/crate/krate/latest/target-redirect/{OTHER_TARGET}/path_add/static.html")
        );
    }

    #[test]
    fn test_debug_output() {
        let params = RustdocParams::new(&DUMMY)
            .try_with_req_version("0.2.0")
            .unwrap()
            .with_inner_path("struct.Dummy.html")
            .with_doc_target("dummy")
            .with_page_kind(PageKind::Rustdoc)
            .try_with_original_uri("/dummy/0.2.0/dummy/struct.Dummy.html")
            .unwrap()
            .with_default_target(DEFAULT_TARGET)
            .with_target_name("dummy")
            .with_doc_targets(TARGETS.iter().cloned());

        let debug_output = format!("{:?}", params);

        assert!(debug_output.contains("EscapedURI"));
        assert!(debug_output.contains("rustdoc_url()"));
        assert!(debug_output.contains("generate_fallback_url()"));
    }

    #[test]
    fn test_override_doc_target_when_old_doc_target_was_path() {
        // params as if they would have come from a route like
        // `/{name}/{version}/{target}/{*path}`,
        // where in the `{target}` place we have part of the path.
        let params = RustdocParams::new(KRATE)
            .with_req_version(ReqVersion::Exact(VERSION))
            .try_with_original_uri("/dummy/0.1.0/dummy/struct.Dummy.html")
            .unwrap()
            .with_doc_target("dummy")
            .with_inner_path("struct.Dummy.html");

        dbg!(&params);

        // initial params, doc-target is "dummy", not validated
        assert_eq!(params.doc_target(), Some("dummy"));
        assert_eq!(params.inner_path(), "struct.Dummy.html");

        // after parsing, we recognize that the doc target is not a target, and attach
        // it to the inner_path.
        let params = params
            .with_default_target(DEFAULT_TARGET)
            .with_target_name(KRATE.to_string())
            .with_doc_targets(TARGETS.iter().cloned());

        dbg!(&params);

        assert_eq!(params.doc_target(), None);
        assert_eq!(params.inner_path(), "dummy/struct.Dummy.html");

        // now, in some cases, we now want to generate a variation of these params,
        // with an actual non-default doc target.
        // Then we expect the path to be intact still, and the target to be set, even
        // though the folder-part of the path was initially generated from the doc_target field.
        let params = params.with_doc_target(OTHER_TARGET);
        dbg!(&params);
        assert_eq!(params.doc_target(), Some(OTHER_TARGET));
        assert_eq!(params.inner_path(), "dummy/struct.Dummy.html");
    }

    #[test]
    fn test_if_order_matters_1() {
        let params = RustdocParams::new(KRATE)
            .with_req_version(ReqVersion::Exact(VERSION))
            .try_with_original_uri("/dummy/0.1.0/dummy/struct.Dummy.html")
            .unwrap()
            .with_inner_path("dummy/struct.Dummy.html")
            .with_default_target(DEFAULT_TARGET)
            .with_target_name(KRATE.to_string())
            .with_doc_targets(TARGETS.iter().cloned());

        assert_eq!(params.doc_target(), None);
        assert_eq!(params.inner_path(), "dummy/struct.Dummy.html");

        let params = params.with_doc_target(OTHER_TARGET);
        assert_eq!(params.doc_target(), Some(OTHER_TARGET));
        assert_eq!(params.inner_path(), "dummy/struct.Dummy.html");
    }

    #[test]
    fn test_if_order_matters_2() {
        let params = RustdocParams::new(KRATE)
            .with_req_version(ReqVersion::Exact(VERSION))
            .try_with_original_uri(format!(
                "/dummy/0.1.0/{OTHER_TARGET}/dummy/struct.Dummy.html"
            ))
            .unwrap()
            .with_inner_path(format!("{OTHER_TARGET}/dummy/struct.Dummy.html"))
            .with_default_target(DEFAULT_TARGET)
            .with_target_name(KRATE.to_string())
            .with_doc_targets(TARGETS.iter().cloned());

        assert_eq!(params.doc_target(), Some(OTHER_TARGET));
        assert_eq!(params.inner_path(), "dummy/struct.Dummy.html");

        let params = params.with_doc_target(DEFAULT_TARGET);
        assert_eq!(params.doc_target(), Some(DEFAULT_TARGET));
        assert_eq!(params.inner_path(), "dummy/struct.Dummy.html");
    }

    #[test]
    fn test_parse_something() {
        // test for https://github.com/rust-lang/docs.rs/issues/2989
        let params = dbg!(
            RustdocParams::new(KRATE)
                .with_page_kind(PageKind::Rustdoc)
                .try_with_original_uri(format!("/{KRATE}/latest/{KRATE}"))
                .unwrap()
                .with_req_version(ReqVersion::Latest)
                .with_doc_target(KRATE.to_string())
        );

        assert_eq!(params.rustdoc_url(), "/krate/latest/krate/");

        let params = dbg!(
            params
                .with_target_name(KRATE.to_string())
                .with_default_target(DEFAULT_TARGET)
                .with_doc_targets(TARGETS.iter().cloned())
        );

        assert_eq!(params.rustdoc_url(), "/krate/latest/krate/");
    }

    #[test_case("other_path.html", "/krate/latest/krate/other_path.html")]
    #[test_case("other_path", "/krate/latest/krate/other_path"; "without .html")]
    #[test_case("other_path.html", "/krate/latest/krate/other_path.html"; "with .html")]
    #[test_case("settings.html", "/krate/latest/settings.html"; "static routes")]
    #[test_case("krate", "/krate/latest/krate/"; "same as target name, without slash")]
    fn test_redirect_some_odd_paths_we_saw(inner_path: &str, expected_url: &str) {
        // test for https://github.com/rust-lang/docs.rs/issues/2989
        let params = RustdocParams::new(KRATE)
            .with_page_kind(PageKind::Rustdoc)
            .try_with_original_uri(format!("/{KRATE}/latest/{inner_path}"))
            .unwrap()
            .with_req_version(ReqVersion::Latest)
            .with_maybe_doc_target(None::<String>)
            .with_inner_path(inner_path)
            .with_default_target(DEFAULT_TARGET)
            .with_target_name(KRATE.to_string())
            .with_doc_targets(TARGETS.iter().cloned());

        dbg!(&params);

        assert_eq!(params.rustdoc_url(), expected_url);
    }

    #[test]
    fn test_item_with_semver_url() {
        // https://github.com/rust-lang/docs.rs/issues/3036
        // This fixes an issue where we mistakenly attached a
        // trailing `/` to a rustdoc URL when redirecting
        // to the exact version, coming from a semver version.

        let ver: Version = "0.14.0".parse().unwrap();
        let params = RustdocParams::new(KRATE)
            .with_page_kind(PageKind::Rustdoc)
            .with_req_version(ReqVersion::Exact(ver))
            .with_doc_target(KRATE.to_string())
            .with_inner_path("trait.Itertools.html");

        dbg!(&params);

        assert_eq!(
            params.rustdoc_url(),
            format!("/{KRATE}/0.14.0/{KRATE}/trait.Itertools.html")
        )
    }

    #[test_case(None)]
    #[test_case(Some(CompressionAlgorithm::Gzip))]
    #[test_case(Some(CompressionAlgorithm::Zstd))]
    fn test_plain_json_url(wanted_compression: Option<CompressionAlgorithm>) {
        let mut params = RustdocParams::new(KRATE)
            .with_page_kind(PageKind::Rustdoc)
            .with_req_version(ReqVersion::Exact(V1));

        assert_eq!(
            params.json_download_url(wanted_compression, None),
            format!(
                "/crate/{KRATE}/{V1}/json{}",
                wanted_compression
                    .map(|c| format!(".{}", c.file_extension()))
                    .unwrap_or_default()
            )
        );

        params = params.with_doc_target("some-target");

        assert_eq!(
            params.json_download_url(wanted_compression, None),
            format!(
                "/crate/{KRATE}/{V1}/some-target/json{}",
                wanted_compression
                    .map(|c| format!(".{}", c.file_extension()))
                    .unwrap_or_default()
            )
        );
    }

    #[test_case(None)]
    #[test_case(Some(CompressionAlgorithm::Gzip))]
    #[test_case(Some(CompressionAlgorithm::Zstd))]
    fn test_plain_json_url_with_format(wanted_compression: Option<CompressionAlgorithm>) {
        let mut params = RustdocParams::new(KRATE)
            .with_page_kind(PageKind::Rustdoc)
            .with_req_version(ReqVersion::Exact(V1));

        assert_eq!(
            params.json_download_url(wanted_compression, Some("42")),
            format!(
                "/crate/{KRATE}/{V1}/json/42{}",
                wanted_compression
                    .map(|c| format!(".{}", c.file_extension()))
                    .unwrap_or_default()
            )
        );

        params = params.with_doc_target("some-target");

        assert_eq!(
            params.json_download_url(wanted_compression, Some("42")),
            format!(
                "/crate/{KRATE}/{V1}/some-target/json/42{}",
                wanted_compression
                    .map(|c| format!(".{}", c.file_extension()))
                    .unwrap_or_default()
            )
        );
    }

    #[test]
    fn test_zip_download_url() {
        let params = RustdocParams::new(KRATE).with_req_version(ReqVersion::Exact(V1));
        assert_eq!(
            params.zip_download_url(),
            format!("/crate/{KRATE}/{V1}/download")
        );
    }
}
