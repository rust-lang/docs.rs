use super::{ByteSize, GB, GIB, KB, KIB, MB, MIB};
use std::fmt;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Format {
    Iec,
    Si,
}

#[derive(Debug, Clone)]
pub struct Display {
    pub(crate) byte_size: ByteSize,
    pub(crate) format: Format,
}

impl Display {
    #[must_use]
    pub fn iec(mut self) -> Self {
        self.format = Format::Iec;
        self
    }

    #[must_use]
    pub fn si(mut self) -> Self {
        self.format = Format::Si;
        self
    }
}

impl fmt::Display for Display {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.byte_size.as_u64();
        let units = match self.format {
            Format::Iec => [(GIB, "GiB"), (MIB, "MiB"), (KIB, "KiB")],
            Format::Si => [(GB, "GB"), (MB, "MB"), (KB, "kB")],
        };
        let precision = f.precision().unwrap_or(1);

        for (factor, suffix) in units {
            if bytes >= factor {
                let size = bytes as f64 / factor as f64;
                return write!(f, "{size:.precision$} {suffix}");
            }
        }
        write!(f, "{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_si_and_iec_units() {
        for (bytes, si, iec) in [
            (0, "0 B", "0 B"),
            (999, "999 B", "999 B"),
            (KB, "1.0 kB", "1000 B"),
            (KIB, "1.0 kB", "1.0 KiB"),
            (MB, "1.0 MB", "976.6 KiB"),
            (MIB, "1.0 MB", "1.0 MiB"),
            (GB, "1.0 GB", "953.7 MiB"),
            (GIB, "1.1 GB", "1.0 GiB"),
            (u64::MAX, "18446744073.7 GB", "17179869184.0 GiB"),
        ] {
            let size = ByteSize(bytes);
            assert_eq!(size.display().si().to_string(), si);
            assert_eq!(size.display().iec().to_string(), iec);
            assert_eq!(size.to_string(), iec);
        }
    }

    #[test]
    fn respects_precision() {
        assert_eq!(format!("{:.3}", ByteSize(1536)), "1.500 KiB");
        assert_eq!(format!("{:.0}", ByteSize(1500).display().si()), "2 kB");
        assert_eq!(format!("{:.3}", ByteSize(42)), "42 B");
    }
}
