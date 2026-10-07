use anyhow::{Context as _, bail};
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub(super) async fn bind(
    addr: SocketAddr,
    mut listenfd: listenfd::ListenFd,
) -> anyhow::Result<TcpListener> {
    // Both the standalone web binary and the legacy daemon use this path.
    // The inherited listener takes precedence over the configured bind address.
    if listenfd.len() > 1 {
        bail!("expected exactly one socket activation descriptor");
    }
    match listenfd
        .take_tcp_listener(0)
        .context("error acquiring socket activation listener")?
    {
        Some(listener) => {
            listener
                .set_nonblocking(true)
                .context("error making socket activation listener nonblocking")?;
            tokio::net::TcpListener::from_std(listener)
                .context("error registering socket activation listener with Tokio")
        }
        None => tokio::net::TcpListener::bind(addr)
            .await
            .context("error binding socket for web server"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::{io::AsyncWriteExt as _, net::TcpStream};

    #[tokio::test]
    async fn binds_and_accepts_without_activation() {
        let listener = bind("127.0.0.1:0".parse().unwrap(), listenfd::ListenFd::empty())
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(addr.ip().is_loopback());
        assert_ne!(addr.port(), 0);

        tokio::time::timeout(Duration::from_secs(5), async {
            let client = TcpStream::connect(addr).await.unwrap();
            let (_, peer) = listener.accept().await.unwrap();
            assert_eq!(peer, client.local_addr().unwrap());
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn reports_bind_failure_without_activation() {
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let error = bind(occupied.local_addr().unwrap(), listenfd::ListenFd::empty())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "error binding socket for web server");
    }

    // Run with:
    // cargo test -p docs_rs_web --lib handlers::listener::tests::systemd_socket_activation -- --ignored
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires systemd-socket-activate and local TCP sockets"]
    fn systemd_socket_activation() {
        use std::{
            io::Read as _,
            process::{Child, Command},
            thread,
            time::Instant,
        };

        const CHILD_ENV: &str = "DOCSRS_SOCKET_ACTIVATION_TEST_CHILD";
        const TEST_NAME: &str = "handlers::listener::tests::systemd_socket_activation";

        if let Ok(expected_addr) = std::env::var(CHILD_ENV) {
            // Read the activation environment before creating the Tokio runtime.
            let listenfd = listenfd::ListenFd::from_env();
            assert_eq!(listenfd.len(), 1);
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let listener = bind("127.0.0.1:0".parse().unwrap(), listenfd)
                        .await
                        .unwrap();
                    assert_eq!(listener.local_addr().unwrap().to_string(), expected_addr);
                    tokio::time::timeout(Duration::from_secs(5), async {
                        let (mut stream, _) = listener.accept().await.unwrap();
                        stream.write_all(b"activated").await.unwrap();
                    })
                    .await
                    .unwrap();
                });
            return;
        }

        // Ensure failed assertions also terminate and reap the activation process.
        struct ActivationProcess(Child);
        impl Drop for ActivationProcess {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = reservation.local_addr().unwrap();
        drop(reservation);

        let mut child = ActivationProcess(
            Command::new("systemd-socket-activate")
                .args([
                    "--listen",
                    &addr.to_string(),
                    "--setenv",
                    &format!("{CHILD_ENV}={addr}"),
                ])
                .env_remove("LISTEN_PID")
                .env_remove("LISTEN_FDS")
                .env_remove("LISTEN_FDS_FIRST_FD")
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", TEST_NAME, "--ignored", "--test-threads=1"])
                .spawn()
                .expect("install systemd-socket-activate to run this test"),
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                panic!("activation process exited before accepting a connection: {status}");
            }
            match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
                Ok(stream) => break stream,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "activation connection failed: {error}"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
            }
        };
        // This connection triggers activation and queues before the child accepts it.
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reply = [0; 9];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"activated");

        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "activation child failed: {status}");
                break;
            }
            assert!(Instant::now() < deadline, "activation child did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
