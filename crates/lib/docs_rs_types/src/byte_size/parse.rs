use super::{ByteSize, GB, GIB, KB, KIB, MB, MIB};
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
#[error("error parsing ByteSize: {0}")]
pub struct ParseByteSizeError(String);

impl FromStr for ByteSize {
    type Err = ParseByteSizeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let split = value
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(value.len());
        let (number, suffix) = value.split_at(split);
        let factor = match suffix.trim().to_ascii_lowercase().as_str() {
            "" | "b" => 1,
            "k" | "kb" => KB,
            "m" | "mb" => MB,
            "g" | "gb" => GB,
            "ki" | "kib" => KIB,
            "mi" | "mib" => MIB,
            "gi" | "gib" => GIB,
            _ => return Err(ParseByteSizeError(format!("unknown size unit: {suffix:?}"))),
        };

        // Keep integer inputs exact, even above f64's integer precision limit.
        if let Ok(number) = number.parse::<u64>() {
            return Ok(Self(number.saturating_mul(factor)));
        }
        let number = number
            .parse::<f64>()
            .map_err(|error| ParseByteSizeError(format!("invalid value {value:?}: {error}")))?;
        // Fractional bytes are truncated; values beyond u64::MAX saturate.
        Ok(Self((number * factor as f64) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_units() {
        for (input, bytes) in [
            ("42", 42),
            ("42 B", 42),
            ("1 kb", KB),
            ("1 MB", MB),
            ("1 g", GB),
            ("1 KiB", KIB),
            ("1 mi", MIB),
            ("1 GIB", GIB),
            (" 1.5 MiB ", MIB + MIB / 2),
            ("0.5 B", 0),
            ("9007199254740993 B", 9_007_199_254_740_993),
            ("18446744073709551615", u64::MAX),
            ("18446744073709551615 GB", u64::MAX),
            ("18446744073709551616 B", u64::MAX),
        ] {
            assert_eq!(input.parse::<ByteSize>().unwrap(), ByteSize(bytes));
        }
    }

    #[test]
    fn rejects_invalid_sizes_and_larger_units() {
        for input in [
            "", "B", "-1 B", "NaN", "inf", "1..2 MB", "1 PB", "1 PiB", "1 XB",
        ] {
            assert!(input.parse::<ByteSize>().is_err(), "accepted {input:?}");
        }
    }
}
