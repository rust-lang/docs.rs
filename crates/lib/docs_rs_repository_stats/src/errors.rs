use core::fmt;

#[derive(Debug)]
pub struct RateLimitReached;

impl core::error::Error for RateLimitReached {}
impl fmt::Display for RateLimitReached {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("rate limit reached")
    }
}
