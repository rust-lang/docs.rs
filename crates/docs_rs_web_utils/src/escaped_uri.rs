use super::{encode_url_path, url_decode};
use askama::filters::HtmlSafe;
use http::{Uri, uri::PathAndQuery};
use std::{borrow::Borrow, fmt::Display, iter, str::FromStr};
use url::form_urlencoded;

/// internal wrapper around `http::Uri` with some convenience functions.
///
/// Ensures that the path part is always properly percent-encoded, including some characters
/// that http::Uri would allow, but we still want to encode, like umlauts.
/// Also ensures that some characters are _not_ encoded that sometimes arrive percent-encoded
/// from browsers, so we then can easily compare URIs, knowing they are encoded the same way.
///
/// Also we support fragments, with http::Uri doesn't support yet.
/// See https://github.com/hyperium/http/issues/775
#[derive(Debug, Clone, PartialEq)]
pub struct EscapedURI {
    uri: Uri,
    fragment: Option<String>,
}

impl bincode::Encode for EscapedURI {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        // encode as separate parts so we don't have to clone
        self.uri.scheme_str().encode(encoder)?;
        self.uri.authority().map(|a| a.as_str()).encode(encoder)?;
        self.uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .encode(encoder)?;
        self.fragment.encode(encoder)?;
        Ok(())
    }
}

impl EscapedURI {
    pub fn from_uri(uri: Uri) -> Self {
        if uri.path_and_query().is_some() {
            let encoded_path = encode_url_path(
                // we re-encode the path so we know all EscapedURI instances are comparable and
                // encoded the same way.
                // Example: "^" is not escaped when axum generates an Uri, we also didn't do it
                // for a long time so we have nicers URLs with caret, since it's supported by
                // most browsers to be shown in the URL bar.
                // But: the actual request will have it encoded, which means the `Uri`
                // we get from axum when handling the request will have it encoded.
                &url_decode(uri.path()).expect("was in Uri, so has to have been correct"),
            );
            if uri.path() == encoded_path {
                Self {
                    uri,
                    fragment: None,
                }
            } else {
                // path needs additional encoding
                let mut parts = uri.into_parts();

                parts.path_and_query = Some(
                    PathAndQuery::from_maybe_shared(
                        parts
                            .path_and_query
                            .take()
                            .map(|pq| {
                                format!(
                                    "{}{}",
                                    encoded_path,
                                    pq.query().map(|q| format!("?{}", q)).unwrap_or_default(),
                                )
                            })
                            .unwrap_or_default(),
                    )
                    .expect("can't fail since we encode the path ourselves"),
                );

                Self {
                    uri: Uri::from_parts(parts)
                        .expect("everything is coming from a previous Uri, or encoded here"),
                    fragment: None,
                }
            }
        } else {
            Self {
                uri,
                fragment: None,
            }
        }
    }

    pub fn from_path(path: impl AsRef<str>) -> Self {
        Self {
            uri: Uri::builder()
                .path_and_query(encode_url_path(path.as_ref()))
                .build()
                .expect("this can never fail because we encode the path"),
            fragment: None,
        }
    }

    pub fn from_path_and_raw_query(
        path: impl AsRef<str>,
        raw_query: Option<impl AsRef<str>>,
    ) -> Self {
        Self::from_path(path).append_raw_query(raw_query)
    }

    #[cfg(test)]
    pub(crate) fn from_path_and_query<P, I, K, V>(path: P, queries: I) -> Self
    where
        P: AsRef<str>,
        I: IntoIterator,
        I::Item: Borrow<(K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        Self::from_path(path).append_query_pairs(queries)
    }

    pub fn scheme(&self) -> Option<&http::uri::Scheme> {
        self.uri.scheme()
    }

    pub fn authority(&self) -> Option<&http::uri::Authority> {
        self.uri.authority()
    }

    pub fn path(&self) -> &str {
        self.uri.path()
    }

    pub fn query(&self) -> Option<&str> {
        self.uri.query()
    }

    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    /// extend the query part of the Uri with the given raw query string.
    ///
    /// Will parse & re-encode the string, which is why the method is infallible (I think)
    pub fn append_raw_query(self, raw_query: Option<impl AsRef<str>>) -> Self {
        let raw_query = match raw_query {
            Some(ref q) => q.as_ref(),
            None => return self,
        };

        self.append_query_pairs(form_urlencoded::parse(raw_query.as_bytes()))
    }

    pub fn append_query_pairs<I, K, V>(self, new_query_args: I) -> Self
    where
        I: IntoIterator,
        I::Item: Borrow<(K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut new_query_args = new_query_args.into_iter().peekable();
        if new_query_args.peek().is_none() {
            return self;
        }

        let mut serializer = form_urlencoded::Serializer::new(String::new());

        if let Some(existing_query_args) = self.uri.query() {
            serializer.extend_pairs(form_urlencoded::parse(existing_query_args.as_bytes()));
        }

        serializer.extend_pairs(new_query_args);

        let mut parts = self.uri.into_parts();

        parts.path_and_query = Some(
            PathAndQuery::from_maybe_shared(format!(
                "{}?{}",
                parts
                    .path_and_query
                    .map(|pg| pg.path().to_owned())
                    .unwrap_or_default(),
                serializer.finish(),
            ))
            .expect("can't fail since all the data is either coming from a previous Uri, or we encode it ourselves")
        );

        Self::from_uri(
            Uri::from_parts(parts).expect(
                "can't fail since data is either coming from an Uri, or encoded by ourselves.",
            ),
        )
    }

    /// extend query part
    pub fn append_query_pair(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.append_query_pairs(iter::once((key, value)))
    }

    pub fn into_inner(self) -> Uri {
        self.uri
    }

    pub fn with_fragment(mut self, fragment: impl AsRef<str>) -> Self {
        self.fragment = Some(encode_url_path(fragment.as_ref()));
        self
    }
}

impl FromStr for EscapedURI {
    type Err = http::uri::InvalidUri;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((base, fragment)) = s.split_once('#') {
            Ok(Self::from_uri(base.parse()?).with_fragment(fragment))
        } else {
            Ok(Self::from_uri(s.parse()?))
        }
    }
}

impl Display for EscapedURI {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.uri)?;
        if let Some(ref fragment) = self.fragment {
            write!(f, "#{}", fragment)?;
        }
        Ok(())
    }
}

impl HtmlSafe for EscapedURI {}

impl TryFrom<EscapedURI> for Uri {
    type Error = anyhow::Error;

    fn try_from(value: EscapedURI) -> Result<Self, Self::Error> {
        if let Some(fragment) = value.fragment {
            Err(anyhow::anyhow!(
                "can't convert EscapedURI with fragment '{}' into Uri",
                fragment
            ))
        } else {
            Ok(value.uri)
        }
    }
}

impl From<Uri> for EscapedURI {
    fn from(value: Uri) -> Self {
        Self::from_uri(value)
    }
}

impl TryFrom<String> for EscapedURI {
    type Error = http::uri::InvalidUri;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl TryFrom<&str> for EscapedURI {
    type Error = http::uri::InvalidUri;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl PartialEq<String> for &EscapedURI {
    fn eq(&self, other: &String) -> bool {
        *self == other
    }
}

impl PartialEq<String> for EscapedURI {
    fn eq(&self, other: &String) -> bool {
        <Self as PartialEq<str>>::eq(self, other)
    }
}

impl PartialEq<&str> for EscapedURI {
    fn eq(&self, other: &&str) -> bool {
        <Self as PartialEq<str>>::eq(self, other)
    }
}

impl PartialEq<str> for EscapedURI {
    fn eq(&self, other: &str) -> bool {
        if let Some((other_uri, other_fragment)) = other.split_once('#') {
            self.uri == other_uri && self.fragment.as_deref() == Some(other_fragment)
        } else {
            self.uri == other && self.fragment.is_none()
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use super::EscapedURI;
//     use crate::web::{cache::CachePolicy, error::AxumNope};
//     use axum::response::IntoResponse as _;
//     use http::Uri;
//     use test_case::test_case;

//     fn test_serialization_roundtrip(input: &EscapedURI) {
//         let s = input.to_string();
//         assert_eq!(input, s); // tests the ParialEq<str> impl
//         assert_eq!(s.parse::<EscapedURI>().unwrap(), *input);
//     }

//     #[test]
//     fn test_redirect_error_encodes_url_path() {
//         let response = AxumNope::Redirect(
//             EscapedURI::from_path("/something>"),
//             CachePolicy::ForeverInCdnAndBrowser,
//         )
//         .into_response();

//         assert_eq!(response.status(), 302);
//         assert_eq!(response.headers().get("Location").unwrap(), "/something%3E");
//     }

//     #[test_case("/something" => "/something")]
//     #[test_case("/something>" => "/something%3E")]
//     fn test_escaped_uri_encodes_from_path(input: &str) -> String {
//         let escaped = EscapedURI::from_path(input);
//         test_serialization_roundtrip(&escaped);
//         escaped.path().to_owned()
//     }

//     #[test_case("/something" => "/something"; "plain path")]
//     #[test_case("/semver/%5E1.2.3" => "/semver/^1.2.3"; "we encode less")]
//     #[test_case("/somethingäöü" => "/something%C3%A4%C3%B6%C3%BC"; "path with umlauts")]
//     fn test_escaped_uri_encodes_path_from_uri(path: &str) -> String {
//         let uri: Uri = path.parse().unwrap();
//         let escaped = EscapedURI::from_uri(uri);
//         test_serialization_roundtrip(&escaped);
//         escaped.path().to_string()
//     }

//     #[test]
//     fn test_escaped_uri_from_uri_with_query_args() {
//         let uri: Uri = "/something?key=value&foo=bar".parse().unwrap();
//         let escaped = EscapedURI::from_uri(uri);
//         test_serialization_roundtrip(&escaped);
//         assert_eq!(escaped.path(), "/something");
//         assert_eq!(escaped.query(), Some("key=value&foo=bar"));
//     }

//     #[test]
//     fn test_escaped_uri_from_uri_with_query_args_and_fragment() {
//         let input = "/something?key=value&foo=bar#frag";
//         let escaped: EscapedURI = input.parse().unwrap();
//         test_serialization_roundtrip(&escaped);
//         assert_eq!(escaped.path(), "/something");
//         assert_eq!(escaped.query(), Some("key=value&foo=bar"));
//         assert_eq!(escaped.fragment(), Some("frag"));
//         assert_eq!(escaped.to_string(), input);
//     }

//     #[test]
//     fn test_escaped_uri_from_uri_with_query_args_and_fragment_to_encode() {
//         let input = "/something?key=value&foo=bar#fräöag";
//         let escaped: EscapedURI = input.parse().unwrap();
//         test_serialization_roundtrip(&escaped);
//         assert_eq!(escaped.path(), "/something");
//         assert_eq!(escaped.query(), Some("key=value&foo=bar"));
//         assert_eq!(escaped.fragment(), Some("fr%C3%A4%C3%B6ag"));
//         assert_eq!(
//             escaped.to_string(),
//             "/something?key=value&foo=bar#fr%C3%A4%C3%B6ag"
//         );
//     }

//     #[test_case("/something>")]
//     #[test_case("/something?key=<value&foo=\rbar")]
//     fn test_escaped_uri_encodes_path_from_uri_invalid(input: &str) {
//         // things that are invalid URIs should error out,
//         // so are unusable for EscapedURI::from_uri`
//         //
//         // More to test if my assumption is correct that we don't have to re-encode.
//         assert!(input.parse::<Uri>().is_err());
//     }

//     #[test_case(
//         "/something", "key=value&foo=bar"
//         => ("/something".into(), "key=value&foo=bar".into());
//         "plain convert"
//     )]
//     #[test_case(
//         "/something", "value=foo\rbar&key=<value"
//         => ("/something".into(), "value=foo%0Dbar&key=%3Cvalue".into());
//         "invalid query gets re-encoded without error"
//     )]
//     fn test_escaped_uri_from_raw_query(path: &str, query: &str) -> (String, String) {
//         let uri = EscapedURI::from_path_and_raw_query(path, Some(query));
//         test_serialization_roundtrip(&uri);

//         (uri.path().to_owned(), uri.query().unwrap().to_owned())
//     }

//     #[test]
//     fn test_escaped_uri_from_query() {
//         let uri =
//             EscapedURI::from_path_and_query("/something", &[("key", "value"), ("foo", "bar")]);
//         test_serialization_roundtrip(&uri);

//         assert_eq!(uri.path(), "/something");
//         assert_eq!(uri.query(), Some("key=value&foo=bar"));
//     }

//     #[test]
//     fn test_escaped_uri_from_query_with_chars_to_encode() {
//         let uri =
//             EscapedURI::from_path_and_query("/something", &[("key", "value>"), ("foo", "\rbar")]);
//         test_serialization_roundtrip(&uri);

//         assert_eq!(uri.path(), "/something");
//         assert_eq!(uri.query(), Some("key=value%3E&foo=%0Dbar"));
//     }

//     #[test]
//     fn test_escaped_uri_append_query_pairs_without_path() {
//         let uri = Uri::builder().build().unwrap();

//         let parts = uri.into_parts();
//         // `append_query_pairs` has a special case when path_and_query is `None`,
//         // which I want to test here.
//         assert!(parts.path_and_query.is_none());

//         // also tests appending query pairs if there are no existing query args
//         let uri = EscapedURI::from_uri(Uri::from_parts(parts).unwrap())
//             .append_query_pairs(&[("foo", "bar"), ("bar", "baz")]);
//         test_serialization_roundtrip(&uri);

//         assert_eq!(uri.path(), "/");
//         assert_eq!(uri.query(), Some("foo=bar&bar=baz"));
//     }

//     #[test]
//     fn test_escaped_uri_append_query_pairs() {
//         let uri = EscapedURI::from_path_and_query("/something", &[("key", "value")])
//             .append_query_pairs(&[("foo", "bar"), ("bar", "baz")])
//             .append_query_pair("last", "one");
//         test_serialization_roundtrip(&uri);

//         assert_eq!(uri.path(), "/something");
//         assert_eq!(uri.query(), Some("key=value&foo=bar&bar=baz&last=one"));
//     }

//     #[test]
//     fn test_escaped_uri_append_fragment() {
//         let uri = EscapedURI::from_path("/something").with_fragment("some-fragment");
//         test_serialization_roundtrip(&uri);

//         assert_eq!(uri.path(), "/something");
//         assert_eq!(uri.query(), None);
//         assert_eq!(uri.fragment(), Some("some-fragment"));
//         assert_eq!(uri.to_string(), "/something#some-fragment");
//     }

//     #[test]
//     fn test_escaped_uri_append_fragment_encode() {
//         let uri = EscapedURI::from_path("/something").with_fragment("some-äö-fragment");
//         test_serialization_roundtrip(&uri);

//         assert_eq!(uri.path(), "/something");
//         assert_eq!(uri.query(), None);
//         assert_eq!(uri.fragment(), Some("some-%C3%A4%C3%B6-fragment"));
//         assert_eq!(uri.to_string(), "/something#some-%C3%A4%C3%B6-fragment");
//     }

//     #[test]
//     fn test_escaped_uri_replace_fragment() {
//         let uri = EscapedURI::from_path("/something")
//             .with_fragment("some-fragment")
//             .with_fragment("other-fragment");

//         test_serialization_roundtrip(&uri);

//         assert_eq!(uri.path(), "/something");
//         assert_eq!(uri.query(), None);
//         assert_eq!(uri.fragment(), Some("other-fragment"));
//         assert_eq!(uri.to_string(), "/something#other-fragment");
//     }

//     #[test]
//     fn test_comparision() {
//         let uri = EscapedURI::from_path("/something").with_fragment("other-fragment");

//         test_serialization_roundtrip(&uri);

//         assert_eq!(uri.path(), "/something");
//         assert_eq!(uri.query(), None);
//         assert_eq!(uri.fragment(), Some("other-fragment"));
//         assert_eq!(uri.to_string(), "/something#other-fragment");
//     }

//     #[test]
//     fn test_not_eq() {
//         let uri = EscapedURI::from_path("/something").with_fragment("other-fragment");
//         assert_ne!(uri, "/something");
//     }
// }
