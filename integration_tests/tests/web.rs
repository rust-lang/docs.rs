use anyhow::{Context as _, Result, bail};
use docs_rs_config::AppConfig;
use docs_rs_context::Context;
use docs_rs_web::{Config, build_context, run_web_server};
use reqwest::Url;
use std::{sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    time,
};
use tracing::debug;

fn web_config() -> Result<Arc<Config>> {
    Ok(Arc::new(Config::test_config()?))
}

/// starts a test webserver.
///
/// Graceful shutdown is not needed, the test runtime will clean up after
/// the test is finished.
async fn start_web_server(config: Arc<Config>, context: Arc<Context>) -> Result<Url> {
    let socket_addr = {
        // find free local random port.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("error binding socket for web server")?;

        listener.local_addr()?
    };

    tokio::spawn(async move {
        run_web_server(Some(socket_addr), config, context)
            .await
            .expect("error starting web server")
    });

    const WAIT_DURATION: Duration = Duration::from_millis(10);
    const WAIT_ATTEMPTS: usize = 20;

    for _attempt in 0..WAIT_ATTEMPTS {
        if TcpStream::connect(socket_addr).await.is_ok() {
            return Ok(Url::parse(&format!("http://{}/", socket_addr))?);
        }
        debug!("waiting for webserver to start...");
        time::sleep(WAIT_DURATION).await;
    }

    bail!(
        "test web server failed to start after {}s",
        (WAIT_DURATION * WAIT_ATTEMPTS as u32).as_secs()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_static_files_from_repo_root() -> Result<()> {
    let config = web_config()?;
    let context = build_context().await?;
    let base_url = start_web_server(config, context).await?;

    assert!(
        reqwest::get(base_url.join("/-/static/menu.js")?)
            .await?
            .error_for_status()?
            .text()
            .await?
            .contains("updateMenuPositionForSubMenu")
    );

    Ok(())
}
