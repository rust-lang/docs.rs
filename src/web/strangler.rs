/// This implements a simplified strangler-service,
/// using code from
/// https://github.com/MidasLamb/axum-strangler/
///
/// because
/// * axum-strangler breaks redirects in our current implementation:
///   https://github.com/MidasLamb/axum-strangler/issues/4
/// * it adds quite some dependencies it only needs for supporting WebSockets (which we don't
///   need).
/// * it has more dependencies itself doesn't need (reqwest)
///
/// We might be able to switch back to using the library when
/// * the host/redirect problem is fixed
/// * websocket suport is hidden behind a feature
use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    extract::RequestParts,
    http::{uri::Authority, Uri},
};
use tower_service::Service;

/// Service that forwards all requests to another service
/// ```ignore
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     let strangler_svc = StranglerService::new(
///         axum::http::uri::Authority::from_static("127.0.0.1:3333"),
///     );
///     let router = axum::Router::new().fallback(strangler_svc);
///     axum::Server::bind(&"127.0.0.1:0".parse()?)
///         .serve(router.into_make_service())
///         # .with_graceful_shutdown(async {
///         # // Shut down immediately
///         # })
///         .await?;
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct StranglerService {
    http_client: hyper::Client<hyper::client::HttpConnector>,
    inner: Arc<InnerStranglerService>,
}

impl StranglerService {
    /// Construct a new `StranglerService`.
    /// The `strangled_authority` is the host & port of the service to be strangled.
    pub fn new(strangled_authority: Authority) -> Self {
        Self {
            http_client: hyper::Client::new(),
            inner: Arc::new(InnerStranglerService {
                strangled_authority,
            }),
        }
    }
}

struct InnerStranglerService {
    strangled_authority: axum::http::uri::Authority,
}

impl Service<axum::http::Request<axum::body::Body>> for StranglerService {
    type Response = axum::response::Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: axum::http::Request<axum::body::Body>) -> Self::Future {
        let http_client = self.http_client.clone();
        let inner = self.inner.clone();

        let fut = forward_call_to_strangled(http_client, inner, req);
        Box::pin(fut)
    }
}

#[tracing::instrument(skip_all, fields(req.path = %req.uri()))] // Note that we set the path to the
                                                                // "full" uri, as host etc gets
                                                                // removed by axum already.
async fn forward_call_to_strangled(
    http_client: hyper::Client<hyper::client::HttpConnector>,
    inner: Arc<InnerStranglerService>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Response, Infallible> {
    tracing::info!("handling a request");
    let mut request_parts = RequestParts::new(req);
    let req: Result<axum::http::Request<axum::body::Body>, _> = request_parts.extract().await;
    let mut req = req.unwrap();

    let uri: Uri = {
        // Not really anything to do, because this could just not be a websocket
        // request.
        let strangled_authority = inner.strangled_authority.clone();
        let strangled_scheme = axum::http::uri::Scheme::HTTP;
        Uri::builder()
            .authority(strangled_authority)
            .scheme(strangled_scheme)
            .path_and_query(req.uri().path_and_query().cloned().unwrap())
            .build()
            .unwrap()
    };

    *req.uri_mut() = uri;

    let r = http_client.request(req).await.unwrap();

    let mut response_builder = axum::response::Response::builder();
    response_builder = response_builder.status(r.status());

    if let Some(headers) = response_builder.headers_mut() {
        *headers = r.headers().clone();
    }

    let response = response_builder
        .body(axum::body::boxed(r))
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR);

    match response {
        Ok(response) => Ok(response),
        Err(_) => todo!(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use axum::{http::uri::Authority, routing::get, Extension, Router};

    /// Create a mock service that's not connecting to anything.
    fn make_svc() -> StranglerService {
        StranglerService::new(Authority::from_static("127.0.0.1:0"))
    }

    #[tokio::test]
    async fn can_be_used_as_fallback() {
        let router = Router::new().fallback(make_svc());
        axum::Server::bind(&"0.0.0.0:0".parse().unwrap()).serve(router.into_make_service());
    }

    #[tokio::test]
    async fn can_be_used_for_a_route() {
        let router = Router::new().route("/api", make_svc());
        axum::Server::bind(&"0.0.0.0:0".parse().unwrap()).serve(router.into_make_service());
    }

    #[derive(Clone)]
    struct StopChannel(Arc<tokio::sync::broadcast::Sender<()>>);

    struct StartupHelper {
        strangler_port: u16,
        strangler_joinhandle: tokio::task::JoinHandle<()>,
        stranglee_joinhandle: tokio::task::JoinHandle<()>,
    }

    async fn start_up_strangler_and_strangled(strangled_router: Router) -> StartupHelper {
        let (tx, mut rx_1) = tokio::sync::broadcast::channel::<()>(1);
        let mut rx_2 = tx.subscribe();
        let tx_arc = Arc::new(tx);
        let stop_channel = StopChannel(tx_arc);

        let stranglee_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stranglee_port = stranglee_tcp.local_addr().unwrap().port();

        let strangler_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let strangler_port = strangler_tcp.local_addr().unwrap().port();

        let client = hyper::Client::new();
        let strangler_svc = StranglerService {
            http_client: client,
            inner: Arc::new(InnerStranglerService {
                strangled_authority: axum::http::uri::Authority::try_from(format!(
                    "127.0.0.1:{}",
                    stranglee_port
                ))
                .unwrap(),
            }),
        };

        let background_stranglee_handle = tokio::spawn(async move {
            axum::Server::from_tcp(stranglee_tcp)
                .unwrap()
                .serve(
                    strangled_router
                        .layer(axum::Extension(stop_channel))
                        .into_make_service(),
                )
                .with_graceful_shutdown(async {
                    rx_1.recv().await.ok();
                })
                .await
                .unwrap();
        });

        let background_strangler_handle = tokio::spawn(async move {
            let router = Router::new().fallback(strangler_svc);
            axum::Server::from_tcp(strangler_tcp)
                .unwrap()
                .serve(router.into_make_service())
                .with_graceful_shutdown(async {
                    rx_2.recv().await.ok();
                })
                .await
                .unwrap();
        });

        StartupHelper {
            strangler_port,
            strangler_joinhandle: background_strangler_handle,
            stranglee_joinhandle: background_stranglee_handle,
        }
    }

    #[tokio::test]
    async fn proxies_strangled_http_service() {
        let router = Router::new().route(
            "/api/something",
            get(
                |Extension(StopChannel(tx_arc)): Extension<StopChannel>| async move {
                    tx_arc.send(()).unwrap();
                    "I'm being strangled"
                },
            ),
        );

        let StartupHelper {
            strangler_port,
            strangler_joinhandle,
            stranglee_joinhandle,
        } = start_up_strangler_and_strangled(router).await;

        let c = reqwest::Client::new();
        let r = c
            .get(format!("http://127.0.0.1:{}/api/something", strangler_port))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();

        assert_eq!(r, "I'm being strangled");

        stranglee_joinhandle.await.unwrap();
        strangler_joinhandle.await.unwrap();
    }
}
