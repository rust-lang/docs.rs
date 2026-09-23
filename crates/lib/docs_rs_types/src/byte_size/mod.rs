mod database;
mod display;
mod ops;
mod parse;

pub use self::display::Display;
use self::display::Format;
pub use parse::ParseByteSizeError;
use std::fmt;

const KB: u64 = 1_000;
const MB: u64 = 1_000_000;
const GB: u64 = 1_000_000_000;

const KIB: u64 = 1_024;
const MIB: u64 = 1_048_576;
const GIB: u64 = 1_073_741_824;

#[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Default)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub const MAX: Self = Self::b(u64::MAX);

    pub const fn b(size: u64) -> ByteSize {
        ByteSize(size)
    }

    pub const fn kb(size: u64) -> ByteSize {
        ByteSize(size * KB)
    }

    pub const fn kib(size: u64) -> ByteSize {
        ByteSize(size * KIB)
    }

    pub const fn mb(size: u64) -> ByteSize {
        ByteSize(size * MB)
    }

    pub const fn mib(size: u64) -> ByteSize {
        ByteSize(size * MIB)
    }

    pub const fn gb(size: u64) -> ByteSize {
        ByteSize(size * GB)
    }

    pub const fn gib(size: u64) -> ByteSize {
        ByteSize(size * GIB)
    }

    pub const fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn as_usize(&self) -> usize {
        self.0
            .try_into()
            .expect("u64 should fit into usize on all systems we run on")
    }

    pub fn as_kb(&self) -> f64 {
        self.0 as f64 / KB as f64
    }

    pub fn as_kib(&self) -> f64 {
        self.0 as f64 / KIB as f64
    }

    pub fn as_mb(&self) -> f64 {
        self.0 as f64 / MB as f64
    }

    pub fn as_mib(&self) -> f64 {
        self.0 as f64 / MIB as f64
    }

    pub fn as_gb(&self) -> f64 {
        self.0 as f64 / GB as f64
    }

    pub fn as_gib(&self) -> f64 {
        self.0 as f64 / GIB as f64
    }

    pub fn display(&self) -> Display {
        Display {
            byte_size: *self,
            format: Format::Iec,
        }
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.display(), f)
    }
}

impl fmt::Debug for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({} bytes)", self, self.0)
    }
}

impl From<u64> for ByteSize {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<usize> for ByteSize {
    fn from(value: usize) -> Self {
        Self(value as u64)
    }
}
