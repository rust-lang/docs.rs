use anyhow::Result;
use docs_rs_utils::{APP_USER_AGENT, spawn_blocking};
use futures_util::StreamExt as _;
use std::{fmt, sync::LazyLock};
use tokio::{
    fs,
    io::{AsyncSeekExt as _, AsyncWriteExt as _},
};
use tracing::debug;

pub(crate) const DOCS_RS: &str = "https://docs.rs";
pub(crate) static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(APP_USER_AGENT)
        .build()
        .expect("can't create request client & connection pool")
});

pub(crate) async fn download(url: impl reqwest::IntoUrl + fmt::Debug) -> Result<Vec<u8>> {
    debug!("downloading...");

    Ok(CLIENT
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec())
}

pub(crate) async fn download_to_temp_file(url: impl reqwest::IntoUrl) -> Result<fs::File> {
    debug!("downloading to temp file..");

    let response = CLIENT.get(url).send().await?.error_for_status()?;

    // NOTE: even after being convert to a `tokio::fs::File`, this kind of temporary file
    // will be cleaned up by the OS, when the last handle is closed.
    let mut file = fs::File::from_std(spawn_blocking(|| Ok(tempfile::tempfile()?)).await?);

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
    }

    file.sync_all().await?;
    file.seek(std::io::SeekFrom::Start(0)).await?;
    Ok(file)
}
