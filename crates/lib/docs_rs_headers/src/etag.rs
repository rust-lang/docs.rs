use headers::ETag;
use std::io::{self, Write};

/// compute our etag header value from some content
///
/// Has to match the implementation in our build-script.
pub fn compute_etag<T: AsRef<[u8]>>(content: T) -> ETag {
    let mut computer = ETagComputer::new();
    computer.write_all(content.as_ref()).unwrap();
    computer.finalize()
}

/// Helper type to compute ETag values.
///
/// Works the same way as the inner `md5::Context`,
/// but produces an `ETag` when finalized.
#[derive(Default)]
pub struct ETagComputer(md5::Context);

impl ETagComputer {
    pub fn new() -> Self {
        Self(md5::Context::new())
    }

    pub fn consume<T: AsRef<[u8]>>(&mut self, data: T) {
        self.0.consume(data.as_ref());
    }

    pub fn finalize(self) -> ETag {
        let digest = self.0.finalize();
        format!("\"{:x}\"", digest).parse().unwrap()
    }
}

impl io::Write for ETagComputer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
