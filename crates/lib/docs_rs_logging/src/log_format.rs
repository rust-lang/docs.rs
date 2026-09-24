use std::{
    fmt,
    io::{self, IsTerminal as _},
    str::FromStr,
};

#[derive(Debug, Clone, Copy)]
pub enum LogFormat {
    Full,
    Compact,
    Pretty,
    Json,
}

impl Default for LogFormat {
    fn default() -> Self {
        if io::stdout().is_terminal() {
            LogFormat::Compact
        } else {
            LogFormat::Json
        }
    }
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
        if s.eq_ignore_ascii_case("full") {
            Ok(Self::Full)
        } else if s.eq_ignore_ascii_case("compact") {
            Ok(Self::Compact)
        } else if s.eq_ignore_ascii_case("pretty") {
            Ok(Self::Pretty)
        } else if s.eq_ignore_ascii_case("json") {
            Ok(Self::Json)
        } else {
            Err(InvalidLogFormat(s.to_string()))
        }
    }
}
