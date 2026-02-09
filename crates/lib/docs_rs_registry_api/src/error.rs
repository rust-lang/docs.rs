use crate::models::ApiErrors;
use reqwest::StatusCode;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid API url")]
    InvalidApiUrl,
    #[error("API error from crates.io: {0}\n{1}")]
    CrateIoApiError(StatusCode, ApiErrors),
    #[error("Error from crates.io: {0}\n{1}")]
    CrateIoError(StatusCode, String),
    #[error("missing releases in crates.io response")]
    MissingReleases,
    #[error("missing metadata in crates.io response")]
    MissingMetadata,
    #[error("HTTP error: {0}\n{1}")]
    HttpError(reqwest::Error, String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// return the HTTP status code of any error inside, if there is any.
    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Self::CrateIoError(status, _) | Self::CrateIoApiError(status, _) => Some(*status),
            Self::HttpError(error, _body) => error.status(),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Self::HttpError(err, String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use reqwest::StatusCode;

    #[test]
    fn test_error_without_status() {
        for err in [
            Error::InvalidApiUrl,
            Error::MissingReleases,
            Error::MissingMetadata,
            Error::Other(anyhow!("some error")),
        ] {
            assert!(err.status().is_none());
        }
    }

    #[test]
    fn test_error_with_included_status() {
        let status = StatusCode::INTERNAL_SERVER_ERROR;

        assert!(Error::CrateIoApiError(status, ApiErrors::default()).status() == Some(status));

        assert!(Error::CrateIoError(status, "".into()).status() == Some(status));
    }

    #[tokio::test]
    async fn test_error_reqwest_error_status() -> Result<()> {
        let status = StatusCode::INTERNAL_SERVER_ERROR;

        let mut srv = mockito::Server::new_async().await;
        let _m = srv
            .mock("GET", "/")
            .with_status(status.as_u16().into())
            .create_async()
            .await;

        let err = reqwest::get(&srv.url())
            .await?
            .error_for_status()
            .unwrap_err();

        assert_eq!(err.status(), Some(status));

        Ok(())
    }
}
