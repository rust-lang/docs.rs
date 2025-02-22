use std::io::{Error as IoError, ErrorKind, Write};

pub(crate) struct SizedBuffer {
    inner: Vec<u8>,
    limit: usize,
}

impl SizedBuffer {
    pub(crate) fn new(limit: usize) -> Self {
        SizedBuffer {
            inner: Vec::new(),
            limit,
        }
    }

    pub(crate) fn reserve(&mut self, amount: usize) {
        if self.inner.len() + amount > self.limit {
            self.inner.reserve_exact(self.limit - self.inner.len());
        } else {
            self.inner.reserve(amount);
        }
    }

    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.inner
    }
}

impl Write for SizedBuffer {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
        if self.inner.len() + buf.len() > self.limit {
            Err(IoError::new(
                ErrorKind::Other,
                crate::error::SizeLimitReached,
            ))
        } else {
            self.inner.write(buf)
        }
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sized_buffer() {
        let mut buffer = SizedBuffer::new(1024);

        // Add two chunks of 500 bytes
        assert_eq!(500, buffer.write(&[0; 500]).unwrap());
        assert_eq!(500, buffer.write(&[0; 500]).unwrap());

        // Ensure adding a third chunk fails
        let error = buffer.write(&[0; 500]).unwrap_err();
        assert!(
            error
                .get_ref()
                .unwrap()
                .is::<crate::error::SizeLimitReached>()
        );

        // Ensure all the third chunk was discarded
        assert_eq!(1000, buffer.inner.len());

        // Ensure it's possible to reach the limit
        assert_eq!(24, buffer.write(&[0; 24]).unwrap());
        assert_eq!(1024, buffer.inner.len());
    }
}
