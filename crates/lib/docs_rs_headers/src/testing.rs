use headers::{self, Header, HeaderMapExt};
use http::{HeaderMap, HeaderValue};

pub(crate) fn test_typed_decode<H, V>(value: V) -> Result<Option<H>, headers::Error>
where
    H: Header,
    V: TryInto<HeaderValue>,
    <V as TryInto<http::HeaderValue>>::Error: std::fmt::Debug,
{
    let mut map = HeaderMap::new();
    map.append(
        H::name(),
        // this `.try_into` only generates the `HeaderValue` items.
        value.try_into().unwrap(),
    );
    // parsing errors from the typed header end up here.
    map.typed_try_get()
}

pub(crate) fn test_typed_encode<H: Header>(header: H) -> HeaderValue {
    let mut map = HeaderMap::new();
    map.typed_insert(header);
    map.get(H::name()).cloned().unwrap()
}
