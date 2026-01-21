//! Structs to build Surrogate-Key header for Fastly CDN
//! see
//! https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Key/haeders.surrogate keys

use anyhow::{Context as _, bail};
use docs_rs_types::KrateName;
use headers::{self, Header};
use http::{HeaderName, HeaderValue};
use itertools::Itertools as _;
use std::{collections::BTreeSet, fmt::Display, iter, str::FromStr};

pub static SURROGATE_KEY: HeaderName = HeaderName::from_static("surrogate-key");

/// a single surrogate key.
///
/// The typical Fastly `Surrogate-Key` header might include more than one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurrogateKey(HeaderValue);

impl core::ops::Deref for SurrogateKey {
    type Target = HeaderValue;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SurrogateKey {
    pub const fn from_static(s: &'static str) -> Self {
        if s.is_empty() {
            panic!("surrogate key cannot be empty");
        }
        if s.len() > 1024 {
            panic!("surrogate key exceeds maximum length of 1024 bytes");
        }

        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b < 0x21 || b > 0x7E {
                panic!("invalid character found in surrogate key");
            }
            i += 1;
        }

        SurrogateKey(HeaderValue::from_static(s))
    }
}

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

/// A full Fastly Surrogate-Key header, containing zero or more keys.
#[derive(Debug, PartialEq, Clone, Default)]
pub struct SurrogateKeys(BTreeSet<SurrogateKey>);

impl IntoIterator for SurrogateKeys {
    type Item = SurrogateKey;
    type IntoIter = <BTreeSet<SurrogateKey> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl From<SurrogateKey> for SurrogateKeys {
    fn from(key: SurrogateKey) -> Self {
        SurrogateKeys(BTreeSet::from_iter(vec![key]))
    }
}

impl From<KrateName> for SurrogateKeys {
    fn from(name: KrateName) -> Self {
        SurrogateKey::from(name).into()
    }
}

impl From<&KrateName> for SurrogateKeys {
    fn from(name: &KrateName) -> Self {
        SurrogateKey::from(name.clone()).into()
    }
}

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

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i http::HeaderValue>,
    {
        let Some(value) = values.next() else {
            return Err(headers::Error::invalid());
        };

        let Ok(value) = value.to_str() else {
            return Err(headers::Error::invalid());
        };

        let keys = value
            .split(' ')
            .map(SurrogateKey::from_str)
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|_| headers::Error::invalid())?;

        Ok(SurrogateKeys(keys))
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
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    // Build SurrogateKeys from an iterator, de-duplicating keys.
    // Takes only as many elements as would fit into the header,
    // then stops consuming the iterator.
    pub fn extend_until_full<I>(&mut self, iter: I)
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

        let mut current_key_size: u64 = self.encoded_len();

        self.0.extend(iter.into_iter().take_while(|key| {
            let key_size = key.len() as u64 + 1; // +1 for the space or terminator
            if current_key_size + key_size > MAX_LEN {
                false
            } else {
                current_key_size += key_size;
                true
            }
        }))
    }

    pub fn from_iter_until_full<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = SurrogateKey>,
    {
        let mut keys = SurrogateKeys::new();
        keys.extend_until_full(iter);
        keys
    }

    pub fn try_extend<I>(&mut self, iter: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = SurrogateKey>,
    {
        let mut iter = iter.into_iter().peekable();

        self.extend_until_full(&mut iter);

        if iter.peek().is_some() {
            bail!("adding surrogate key would exceed maximum header length of 16,384 bytes");
        }

        Ok(())
    }

    #[cfg(feature = "testing")]
    pub fn try_from_iter<I>(iter: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = SurrogateKey>,
    {
        let mut keys = SurrogateKeys::new();
        keys.try_extend(iter)?;
        Ok(keys)
    }

    pub fn encoded_len(&self) -> u64 {
        self.0.iter().map(|k| (k.0.len() + 1) as u64).sum::<u64>()
    }

    pub fn key_count(&self) -> usize {
        self.0.len()
    }
}

#[cfg(feature = "testing")]
impl FromStr for SurrogateKeys {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let keys = s
            .split(' ')
            .map(SurrogateKey::from_str)
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(SurrogateKeys(keys))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{test_typed_decode, test_typed_encode};
    use std::ops::RangeInclusive;
    use test_case::test_case;

    #[test]
    fn test_parse_surrogate_key_too_long() {
        let input = "X".repeat(1025);
        assert!(SurrogateKey::from_str(&input).is_err());
    }

    #[test]
    #[should_panic]
    fn test_parse_surrogate_key_too_long_const() {
        const INPUT: [u8; 1025] = [b'X'; 1025];
        let input = std::str::from_utf8(&INPUT).unwrap();
        SurrogateKey::from_static(input);
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
    fn test_parse_surrogate_key_ok(input: &'static str) {
        assert_eq!(SurrogateKey::from_str(input).unwrap(), input);
        assert_eq!(SurrogateKey::from_static(input), input);
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
            "key-1 key-2"
        );

        Ok(())
    }

    #[test]
    fn test_decode() -> anyhow::Result<()> {
        assert_eq!(
            test_typed_decode::<SurrogateKeys, _>("key-1 key-2 key-2")?.unwrap(),
            SurrogateKeys::from_iter_until_full([
                SurrogateKey::from_str("key-2").unwrap(),
                SurrogateKey::from_str("key-1").unwrap(),
            ]),
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
