use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::{borrow::Cow, fmt, str::FromStr};

const SEARCH_ARG_QUERY: &str = "q";
const SEARCH_ARG_SORT: &str = "sort";
const SEARCH_ARG_PER_PAGE: &str = "per_page";
const SEARCH_ARG_PAGE: &str = "page";

/// Crate-level metadata returned by [`crate::RegistryApi`].
#[derive(Debug)]
pub struct CrateData {
    /// Owners reported by the registry API.
    pub owners: Vec<CrateOwner>,
}

/// Metadata for one published crate version.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub struct ReleaseData {
    /// Time at which the version was published.
    pub release_time: Option<DateTime<Utc>>,
    /// Whether the version has been yanked.
    pub yanked: bool,
}

/// The `ReleaseData` fields represent what we get from the registry / index.
///
/// But: in many places all over the codebase, we might expect data in
/// `releases.yanked` or `releases.release_time` when a build is finished.
///
/// So for now we explicitly generate dummy data and insert it.
impl ReleaseData {
    pub fn dummy() -> Self {
        ReleaseData {
            release_time: Some(Utc::now()),
            yanked: false,
        }
    }

    /// `ReleaseData` with dummy values inserted if needed.
    pub fn for_database(mut self) -> Self {
        if self.release_time.is_none() {
            self.release_time = Self::dummy().release_time;
        }

        self
    }
}

/// An owner of a crate as reported by the registry API.
#[derive(Debug, Clone)]
pub struct CrateOwner {
    /// URL of the owner's avatar, if the API supplied one.
    pub avatar: String,
    /// The owner's registry login.
    pub login: String,
    /// Whether this owner is a user or a team.
    pub kind: OwnerKind,
}

/// Kind of a crate owner.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    sqlx::Type,
    strum::Display,
)]
#[sqlx(type_name = "owner_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum OwnerKind {
    /// An individual registry user.
    User,
    /// A registry team.
    Team,
}

#[derive(Deserialize, Debug, Default)]
#[cfg_attr(any(test, feature = "testing"), derive(Serialize))]
pub(crate) struct SearchResponse {
    pub(crate) crates: Option<Vec<SearchCrate>>,
    pub(crate) meta: Option<SearchMeta>,
}

/// A crate returned by a registry search.
#[derive(Deserialize, Debug)]
#[cfg_attr(any(test, feature = "testing"), derive(Serialize))]
#[cfg_attr(test, derive(PartialEq, Clone))]
pub struct SearchCrate {
    /// Name of the matching crate.
    pub name: String,
}

/// Pagination cursors returned by a registry search.
#[derive(Deserialize, Debug)]
#[cfg_attr(any(test, feature = "testing"), derive(Serialize))]
#[cfg_attr(test, derive(PartialEq, Clone))]
pub struct SearchMeta {
    /// Cursor for the next result page, if one exists.
    pub(crate) next_page: Option<SearchCursor>,
    /// Cursor for the previous result page, if one exists.
    pub(crate) prev_page: Option<SearchCursor>,
}

impl SearchMeta {
    /// Return the next-page cursor as a typed value.
    pub fn next_page(&self) -> Option<&SearchCursor> {
        self.next_page.as_ref()
    }

    /// Return the previous-page cursor as a typed value.
    pub fn prev_page(&self) -> Option<&SearchCursor> {
        self.prev_page.as_ref()
    }
}

/// An opaque cursor returned by the registry search API for a subsequent result page.
///
/// The registry owns the cursor's query parameters. Callers can inspect the search query and
/// sort order needed for display, but can't change it.
#[derive(Debug, Clone, PartialEq, Eq, SerializeDisplay, DeserializeFromStr)]
pub struct SearchCursor(String);

impl<'a> SearchCursor {
    pub fn parameters(&'a self) -> impl Iterator<Item = (Cow<'a, str>, Cow<'a, str>)> {
        url::form_urlencoded::parse(self.0.trim_start_matches('?').as_bytes())
    }

    fn parameter(&'a self, name: &str) -> Option<Cow<'a, str>> {
        self.parameters()
            .find_map(|(key, value)| (key == name).then_some(value))
    }

    fn parse_parameter<T>(&'a self, name: &str) -> Result<Option<T>>
    where
        T: FromStr,
        T::Err: fmt::Debug,
    {
        let Some(value) = self.parameter(name) else {
            return Ok(None);
        };

        Ok(Some(value.parse().map_err(|err| {
            Error::InvalidSearchCursor(format!(
                "unknown valid in \"{}\" argument: {}\n{:?}",
                name, value, err
            ))
        })?))
    }

    /// Return the original query parameters supplied by the registry.
    pub fn as_params(&self) -> &str {
        &self.0
    }

    /// Return the search term embedded in this cursor, if one is present.
    pub fn query(&'a self) -> Option<Cow<'a, str>> {
        self.parameter(SEARCH_ARG_QUERY)
    }

    /// Return the `page` number
    pub fn page(&'a self) -> Result<Option<u32>> {
        self.parse_parameter(SEARCH_ARG_PAGE)
    }

    /// Return the `per_page` number
    pub fn per_page(&'a self) -> Result<Option<u32>> {
        self.parse_parameter(SEARCH_ARG_PER_PAGE)
    }

    /// Return the sort order embedded in this cursor, if one is present.
    pub fn sort_by(&self) -> Result<Option<SearchSort>> {
        self.parse_parameter(SEARCH_ARG_SORT)
    }

    /// `url::Url` needs the query without the leading `?`
    pub(crate) fn query_for_url(&self) -> &str {
        self.0.trim_start_matches('?')
    }
}

/// `SearchCursor::builder`, only for tests.
///
/// In prod code, the `SearchCursor` object is always created from a
/// crates.io API response, via `FromStr`.
///
/// Ensures that there are no duplicates, but keeps the intersion order.
/// This is helpful when using `.adapt()`.
#[cfg(any(test, feature = "testing"))]
pub struct SearchCursorBuilder {
    args: Vec<(String, String)>,
}

#[cfg(any(test, feature = "testing"))]
impl SearchCursorBuilder {
    pub fn custom_arg(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        let value = value.into();

        // if we already have the key in our cursor, update the value.
        if let Some((_, current_value)) = self.args.iter_mut().find(|(key, _)| key == &name) {
            *current_value = value;
        } else {
            self.args.push((name, value));
        }

        self
    }

    pub fn query(self, query: impl Into<String>) -> Self {
        self.custom_arg(SEARCH_ARG_QUERY, query)
    }

    pub fn sort_by(self, sort_by: SearchSort) -> Self {
        self.custom_arg(SEARCH_ARG_SORT, sort_by.id())
    }

    pub fn per_page(self, per_page: u32) -> Self {
        self.custom_arg(SEARCH_ARG_PER_PAGE, per_page.to_string())
    }

    pub fn page(self, page: u32) -> Self {
        self.custom_arg(SEARCH_ARG_PAGE, page.to_string())
    }

    pub fn build(self) -> SearchCursor {
        SearchCursor(format!(
            "?{}",
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(self.args)
                .finish()
        ))
    }
}

#[cfg(any(test, feature = "testing"))]
impl SearchCursor {
    /// create a builder to create a new `SearchCursor` for tests.
    pub fn builder() -> SearchCursorBuilder {
        SearchCursorBuilder { args: Vec::new() }
    }

    /// creates a pre-filled builder from an existing `SearchCursor`.
    ///
    /// Useful when you have an initial `SearchCursor` and then want
    /// to create the `next_page` and `prev_page` cursors just with
    /// one attribute changed, while the rest stays the same.
    pub fn adapt(self) -> SearchCursorBuilder {
        SearchCursorBuilder {
            args: self
                .parameters()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }
}

impl fmt::Display for SearchCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SearchCursor {
    fn as_ref(&self) -> &str {
        self.as_params()
    }
}

impl FromStr for SearchCursor {
    type Err = Error;

    fn from_str(params: &str) -> Result<Self> {
        if params.starts_with('?') {
            Ok(Self(params.into()))
        } else {
            Err(Error::InvalidSearchCursor(
                "registry pagination cursor must start with '?'".into(),
            ))
        }
    }
}

impl From<&SearchQuery> for SearchCursor {
    fn from(query: &SearchQuery) -> Self {
        query.clone().into()
    }
}

impl From<&SearchCursor> for SearchCursor {
    fn from(cursor: &SearchCursor) -> Self {
        cursor.clone()
    }
}

impl From<SearchQuery> for SearchCursor {
    fn from(query: SearchQuery) -> Self {
        format!(
            "?{}",
            serde_urlencoded::to_string(query).expect("always succeeds")
        )
        .parse()
        .expect("SearchQuery always produces a valid cursor")
    }
}

/// The sorting options the crates.io search API offers.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    Copy,
    Default,
    strum::EnumString,
    strum::Display,
    strum::EnumIter,
    strum::IntoStaticStr,
    strum::AsRefStr,
    SerializeDisplay,
    DeserializeFromStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SearchSort {
    #[default]
    Relevance,
    Downloads,
    RecentDownloads,
    RecentUpdates,
    New,
}

impl SearchSort {
    pub fn id(&self) -> &'static str {
        self.into()
    }

    /// Return the human-readable label used by the UI.
    pub fn label(&self) -> &'static str {
        match *self {
            Self::Relevance => "Relevance",
            Self::Downloads => "All-Time Downloads",
            Self::RecentDownloads => "Recent Downloads",
            Self::RecentUpdates => "Recent Updates",
            Self::New => "Newly Added",
        }
    }
}

/// Parameters for an crates.io crate search.
#[derive(Debug, Clone, PartialEq, Eq, bon::Builder, Serialize, Deserialize)]
pub struct SearchQuery {
    #[builder(start_fn, into)]
    #[serde(rename = "q")]
    query: String,

    #[serde(rename = "sort", skip_serializing_if = "Option::is_none")]
    sort_by: Option<SearchSort>,

    #[serde(skip_serializing_if = "Option::is_none")]
    per_page: Option<u32>,
}

impl From<String> for SearchQuery {
    fn from(query: String) -> Self {
        Self::builder(query).build()
    }
}

impl From<&str> for SearchQuery {
    fn from(query: &str) -> Self {
        Self::builder(query).build()
    }
}

/// Results returned by [`crate::RegistryApi::search`].
#[derive(Deserialize, Debug)]
pub struct Search {
    /// Crates matching the requested query.
    pub crates: Vec<SearchCrate>,
    /// Pagination cursors for the result set.
    pub meta: SearchMeta,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(any(test, feature = "testing"), derive(Serialize))]
pub struct ApiErrors {
    pub errors: Vec<ApiError>,
}

impl fmt::Display for ApiErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for error in &self.errors {
            writeln!(f, "{}", error)?;
        }
        Ok(())
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "testing"), derive(Serialize))]
pub struct ApiError {
    pub detail: Option<String>,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            self.detail.as_deref().unwrap_or("Unknown API Error")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn search_cursor_parses_registry_pagination_params() {
        let cursor = SearchCursor::builder()
            .query("some crate")
            .sort_by(SearchSort::RecentUpdates)
            .page(2)
            .custom_arg("opaque", "value/with escapes")
            .build();

        assert_eq!(
            cursor.as_params(),
            "?q=some+crate&sort=recent-updates&page=2&opaque=value%2Fwith+escapes"
        );
        assert_eq!(cursor.query().as_deref(), Some("some crate"));
        assert_eq!(cursor.sort_by().unwrap(), Some(SearchSort::RecentUpdates));
        assert_eq!(
            cursor
                .parameters()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect::<Vec<_>>(),
            vec![
                (SEARCH_ARG_QUERY.into(), "some crate".into()),
                (SEARCH_ARG_SORT.into(), "recent-updates".into()),
                (SEARCH_ARG_PAGE.into(), "2".into()),
                ("opaque".into(), "value/with escapes".into()),
            ]
        );
        let serialized = serde_json::to_string(&cursor).unwrap();
        assert_eq!(serialized, format!("\"{cursor}\""));
        let deserialized: SearchCursor = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.as_params(), cursor.as_params());
    }

    #[test]
    fn search_cursor_rejects_invalid_values() {
        let error = "q=crate".parse::<SearchCursor>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid search cursor: registry pagination cursor must start with '?'"
        );

        let cursor: SearchCursor = "?sort=not-a-sort".parse().unwrap();
        assert!(cursor.sort_by().is_err());
    }

    #[test]
    fn search_cursor_builder_keeps_argument_order() {
        let cursor = SearchCursor::builder()
            .custom_arg("zoo", "0")
            .custom_arg("first", "1")
            .custom_arg("second", "2")
            .custom_arg("third", "3")
            .build();

        assert_eq!(cursor.as_params(), "?zoo=0&first=1&second=2&third=3");
    }

    #[test]
    fn search_cursor_builder_overwrites_an_existing_argument_in_place() {
        let cursor = SearchCursor::builder()
            .custom_arg("first", "old")
            .custom_arg("second", "2")
            .custom_arg("first", "new")
            .build();

        assert_eq!(cursor.as_params(), "?first=new&second=2");
    }

    #[test_case(SearchSort::Relevance, "relevance", "Relevance")]
    #[test_case(SearchSort::Downloads, "downloads", "All-Time Downloads")]
    #[test_case(SearchSort::RecentDownloads, "recent-downloads", "Recent Downloads")]
    #[test_case(SearchSort::RecentUpdates, "recent-updates", "Recent Updates")]
    #[test_case(SearchSort::New, "new", "Newly Added")]
    fn search_sort_has_stable_identifiers_and_labels(sort: SearchSort, id: &str, label: &str) {
        assert_eq!(sort.id(), id);
        assert_eq!(sort.to_string(), id);
        assert_eq!(id.parse::<SearchSort>().unwrap(), sort);
        assert_eq!(sort.label(), label);
        assert_eq!(serde_json::to_string(&sort).unwrap(), format!("\"{id}\""));
    }

    #[test]
    fn search_sort_rejects_unknown_identifier() {
        assert!("oldest".parse::<SearchSort>().is_err());
    }

    #[test]
    fn search_query_serializes_with_defaults_and_custom_values() {
        fn parse(params: &str) -> Vec<(String, String)> {
            url::form_urlencoded::parse(params.as_bytes())
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        }

        let default_query = SearchQuery::from("some crate");
        let default_params = serde_urlencoded::to_string(&default_query).unwrap();
        assert_eq!(
            parse(&default_params),
            vec![(SEARCH_ARG_QUERY.into(), "some crate".into())]
        );

        let custom_query = SearchQuery::builder("some crate")
            .sort_by(SearchSort::RecentUpdates)
            .per_page(50)
            .build();
        let custom_params = serde_urlencoded::to_string(&custom_query).unwrap();
        assert_eq!(
            parse(&custom_params),
            vec![
                (SEARCH_ARG_QUERY.into(), "some crate".into()),
                (SEARCH_ARG_SORT.into(), "recent-updates".into()),
                (SEARCH_ARG_PER_PAGE.into(), "50".into()),
            ]
        );
        assert_eq!(
            SearchCursor::from(custom_query).as_params(),
            format!("?{custom_params}")
        );
    }

    #[test_case(OwnerKind::User, "user")]
    #[test_case(OwnerKind::Team, "team")]
    fn owner_kind_display_and_serde_round_trip(kind: OwnerKind, value: &str) {
        assert_eq!(kind.to_string(), value);
        assert_eq!(
            serde_json::to_string(&kind).unwrap(),
            format!("\"{value}\"")
        );
        assert_eq!(
            serde_json::from_str::<OwnerKind>(&format!("\"{value}\"")).unwrap(),
            kind
        );
    }

    #[test]
    fn search_cursor_requires_a_query_string() {
        let error = "page=2".parse::<SearchCursor>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid search cursor: registry pagination cursor must start with '?'"
        );
        assert_eq!(
            "?page=2".parse::<SearchCursor>().unwrap().as_params(),
            "?page=2"
        );
    }
}
