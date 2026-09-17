use super::ByteSize;
use std::ops;

impl ops::AddAssign<u64> for ByteSize {
    fn add_assign(&mut self, rhs: u64) {
        self.0 += rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_assign() {
        let mut start = ByteSize::b(42);

        start += 3;

        assert_eq!(start, ByteSize::b(45));
    }
}
