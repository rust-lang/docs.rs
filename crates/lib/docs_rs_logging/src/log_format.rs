use std::{fmt, str::FromStr};

#[derive(Debug, Default)]
pub enum LogFormat {
    Json,
    #[default]
    Pretty,
}

#[derive(Debug)]
pub struct InvalidLogFormat(String);

impl fmt::Display for InvalidLogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid log format: {}", self.0)
    }
}

impl std::error::Error for InvalidLogFormat {}

impl FromStr for LogFormat {
    type Err = InvalidLogFormat;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Self::Json),
            "pretty" => Ok(Self::Pretty),
            _ => Err(InvalidLogFormat(s.to_string())),
        }
    }
}
