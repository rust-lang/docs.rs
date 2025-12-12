//! Structs to build Surrogate-Key header for Fastly CDN
//! see
//! https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Key/haeders.surrogate keys

use anyhow::{Context as _, bail};
use derive_more::Deref;
use docs_rs_database::types::krate_name::KrateName;
use headers::{self, Header};
use http::{HeaderName, HeaderValue};
use itertools::Itertools as _;
use std::{fmt::Display, iter, str::FromStr};

pub static SURROGATE_KEY: HeaderName = HeaderName::from_static("surrogate-key");

/// a single surrogate key.
///
/// The typical Fastly `Surrogate-Key` header might include more than one.
#[derive(Debug, Clone, Deref, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurrogateKey(HeaderValue);

impl FromStr for SurrogateKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // From the Fastly documentation:
        //
        // > Surrogate keys must contain only printable ASCII characters
        // > (those between 0x21 and 0x7E, inclusive).
        // > Any invalid keys will be ignored.
        // > Individual keys are limited to 1024 bytes in length, and the
        // > total length of the Surrogate-Key header may not exceed 16,384 bytes.
        // > If either of these limits are reached while parsing a Surrogate-Key header,
        // > the key currently being parsed and all keys following it within the same header
        // > will be ignored.
        //
        // https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Key/

        if s.is_empty() {
            bail!("surrogate key cannot be empty");
        }
        if s.len() > 1024 {
            bail!("surrogate key exceeds maximum length of 1024 bytes");
        }
        if !s.as_bytes().iter().all(|b| (0x21..=0x7E).contains(b)) {
            bail!("invalid character found");
        }

        Ok(SurrogateKey(s.parse().context("invalid header value")?))
    }
}

impl<T> PartialEq<T> for SurrogateKey
where
    T: AsRef<str>,
{
    fn eq(&self, other: &T) -> bool {
        self.0 == other.as_ref()
    }
}

/// Create a surrogate key from a crate name.
impl From<KrateName> for SurrogateKey {
    fn from(value: KrateName) -> Self {
        // valid crate names only contain chars that are also
        // valid in surrogate keys.
        // And all these are also valid in header-values.
        let key = format!("crate-{}", value);
        Self(
            key.parse()
                .expect("crate name that can't be parsed into HeaderValue"),
        )
    }
}

/// A full Fastly Surrogate-Key header, containing one or more keys.
#[derive(Debug, PartialEq)]
pub struct SurrogateKeys(Vec<SurrogateKey>);

impl Display for SurrogateKeys {
    #[allow(unstable_name_collisions)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for key_or_space in self
            .0
            .iter()
            .map(|key| {
                key.0.to_str().expect(
                "single SurrogateKeys can only be created from strings, so this always succeeds",
            )
            })
            .intersperse(" ")
        {
            write!(f, "{}", key_or_space)?;
        }
        Ok(())
    }
}

impl Header for SurrogateKeys {
    fn name() -> &'static http::HeaderName {
        &SURROGATE_KEY
    }

    fn decode<'i, I>(_values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i http::HeaderValue>,
    {
        unimplemented!();
    }

    fn encode<E: Extend<http::HeaderValue>>(&self, values: &mut E) {
        let header_value: HeaderValue = self.to_string().parse().expect(
            "we know the single keys are valid HeaderValue, and valid Strings,
                  so after joining using spaces, the joined version one must be valid too",
        );
        values.extend(iter::once(header_value))
    }
}

impl SurrogateKeys {
    /// Build SurrogateKeys from an iterator, de-duplicating keys.
    /// Takes only as many elements as would fit into the header,
    /// then stops consuming the iterator.
    pub fn from_iter_until_full<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = SurrogateKey>,
    {
        // From the Fastly documentation:
        //
        // > [...] and the total length of the Surrogate-Key header may not
        // > exceed 16,384 bytes.
        //
        // https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Key/

        const MAX_LEN: u64 = 16_384;

        let mut current_key_size: u64 = 0;

        SurrogateKeys(
            iter.into_iter()
                .unique()
                .take_while(|key| {
                    let key_size = key.len() as u64 + 1; // +1 for the space or terminator
                    if current_key_size + key_size > MAX_LEN {
                        false
                    } else {
                        current_key_size += key_size;
                        true
                    }
                })
                .collect(),
        )
    }

    #[cfg(test)]
    pub fn encoded_len(&self) -> usize {
        self.0.iter().map(|k| k.0.len() + 1).sum::<usize>()
    }

    pub fn key_count(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
impl FromStr for SurrogateKeys {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let keys = s
            .split(' ')
            .map(SurrogateKey::from_str)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SurrogateKeys(keys))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::headers::test_typed_encode;
    use std::ops::RangeInclusive;
    use test_case::test_case;

    #[test]
    fn test_parse_surrogate_key_too_long() {
        let input = "X".repeat(1025);
        assert!(SurrogateKey::from_str(&input).is_err());
    }

    #[test_case(""; "empty")]
    #[test_case(" "; "space")]
    #[test_case("\n"; "newline")]
    fn test_parse_surrogate_key_err(input: &str) {
        assert!(SurrogateKey::from_str(input).is_err());
    }

    #[test_case("some-key")]
    #[test_case("1234")]
    #[test_case("crate-some-crate")]
    #[test_case("release-some-crate-1.2.3")]
    fn test_parse_surrogate_key_ok(input: &str) {
        assert_eq!(SurrogateKey::from_str(input).unwrap(), input);
    }

    #[test]
    fn test_encode() -> anyhow::Result<()> {
        let k1 = SurrogateKey::from_str("key-2").unwrap();
        let k2 = SurrogateKey::from_str("key-1").unwrap();
        // this key is duplicate, should be removed
        let k3 = SurrogateKey::from_str("key-2").unwrap();

        assert_eq!(k1, k3);
        assert_ne!(k1, k2);
        assert_ne!(k3, k2);

        assert_eq!(
            test_typed_encode(SurrogateKeys::from_iter_until_full([k1, k2, k3])),
            "key-2 key-1"
        );

        Ok(())
    }

    #[test_case('0'..='9'; "numbers")]
    #[test_case('a'..='z'; "lower case")]
    #[test_case('A'..='Z'; "upper case")]
    fn test_from_krate_name(range: RangeInclusive<char>) {
        // ensure that the valid character range for crate names also fits
        // into surrogate keys, and header values.
        for ch in range {
            let name = format!("k{}", ch);
            let krate_name: KrateName = name.parse().unwrap();
            let surrogate_key: SurrogateKey = krate_name.into();
            assert_eq!(surrogate_key, format!("crate-{name}"));
        }
    }

    #[test]
    fn test_try_from_iter_checks_full_length() -> anyhow::Result<()> {
        let mut it = (0..10_000).map(|n| SurrogateKey::from_str(&format!("key-{n}")).unwrap());

        let first_key = SurrogateKeys::from_iter_until_full(&mut it);
        assert_eq!(first_key.encoded_len(), 16377); // < the max length of 16384

        // elements remaining in the iterator
        assert_eq!(it.count(), 8056);

        Ok(())
    }
}
