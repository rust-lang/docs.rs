pub trait ByteSizeExt {
    const MAX: Self;
}

impl ByteSizeExt for bytesize::ByteSize {
    const MAX: Self = Self::b(u64::MAX);
}
