//! Serving an axum router over HTTP/1.1 with the limits a server facing the internet needs: a
//! cap on open connections, and timeouts on a connection's handshake, on reading a request's
//! headers and on handling a request, so slow clients can't hold connections open. Shared by
//! the tracker and the engine's API.

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tower::ServiceExt;

/// Limits on the connections a server accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpLimits {
    /// The most connections open at once, including those still in their handshake.
    /// Connections beyond it are closed as they arrive.
    pub max_connections: usize,
    /// How long a client has to complete a connection's handshake, such as TLS's, and then to
    /// send each request's headers, including while a kept-alive connection sits idle.
    pub header_timeout: Duration,
    /// How long a request may take from its headers to its response's headers, including
    /// reading its body. A response's body, such as an event stream, may take longer.
    pub request_timeout: Duration,
    /// How long shutting down waits for requests in progress.
    pub shutdown_grace: Duration,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            max_connections: 10_000,
            header_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(30),
            shutdown_grace: Duration::from_secs(10),
        }
    }
}

/// How long to pause accepting after an error such as running out of file descriptors.
const ACCEPT_ERROR_PAUSE: Duration = Duration::from_millis(100);

/// Serves `router` over plain TCP on `listener` until `shutdown` completes, then waits up to
/// [`HttpLimits::shutdown_grace`] for requests in progress. Handlers can extract
/// `ConnectInfo<SocketAddr>`.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    limits: HttpLimits,
    shutdown: impl Future<Output = ()>,
) {
    let plain = |stream| std::future::ready(Ok(stream));
    serve_with(listener, plain, router, limits, shutdown).await;
}

/// Like [`serve`], but each accepted connection first goes through `handshake`, such as a TLS
/// handshake, within [`HttpLimits::header_timeout`]. Connections whose handshake fails or
/// takes too long are closed.
pub async fn serve_with<H, F, S>(
    listener: TcpListener,
    handshake: H,
    router: Router,
    limits: HttpLimits,
    shutdown: impl Future<Output = ()>,
) where
    H: Fn(TcpStream) -> F,
    F: Future<Output = io::Result<S>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let timeout = limits.request_timeout;
    let router = router.layer(middleware::from_fn(move |request, next| {
        within(timeout, request, next)
    }));
    let connections = Arc::new(Semaphore::new(limits.max_connections));
    let graceful = GracefulShutdown::new();
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header_timeout);

    tokio::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Such as running out of file descriptors: wait for some to close.
                    tracing::warn!(%error, "cannot accept a connection");
                    tokio::time::sleep(ACCEPT_ERROR_PAUSE).await;
                    continue;
                }
            },
        };
        // Over the cap, the connection is dropped, which closes it.
        let Ok(permit) = connections.clone().try_acquire_owned() else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let shaking = tokio::time::timeout(limits.header_timeout, handshake(stream));
        let service = router.clone().map_request(move |mut request: Request<_>| {
            request.extensions_mut().insert(ConnectInfo(peer));
            request.map(Body::new)
        });
        let (builder, watcher) = (builder.clone(), graceful.watcher());
        tokio::spawn(async move {
            if let Ok(Ok(stream)) = shaking.await {
                let service = TowerToHyperService::new(service);
                let connection = builder.serve_connection(TokioIo::new(stream), service);
                // Errors are the client's: a malformed request, a timeout or a reset.
                let _ = watcher.watch(connection).await;
            }
            drop(permit);
        });
    }
    drop(listener);
    let _ = tokio::time::timeout(limits.shutdown_grace, graceful.shutdown()).await;
}

/// Runs a request with a time limit, answering 408 if it takes too long.
async fn within(timeout: Duration, request: Request, next: Next) -> Response {
    match tokio::time::timeout(timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) => timed_out(),
    }
}

/// The 408 answer, in the JSON error shape the tracker's and the engine's APIs share.
fn timed_out() -> Response {
    let body = serde_json::json!({ "error": "timeout", "message": "the request took too long" });
    let content_type = HeaderValue::from_static("application/json");
    let headers = [(header::CONTENT_TYPE, content_type)];
    (StatusCode::REQUEST_TIMEOUT, headers, body.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn router() -> Router {
        Router::new()
            .route(
                "/peer",
                get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
                    peer.ip().to_string()
                }),
            )
            .route("/slow", get(|| tokio::time::sleep(Duration::from_secs(60))))
            .route("/large", get(|| async { vec![7u8; 8 << 20] }))
    }

    async fn start(limits: HttpLimits) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve(listener, router(), limits, std::future::pending()));
        addr
    }

    /// Reads until the server closes the connection, failing if it takes over 5 seconds.
    async fn read_to_close(stream: &mut TcpStream) -> String {
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
            .await
            .expect("the server closes the connection")
            .unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    #[tokio::test]
    async fn serves_with_the_peer_address() {
        let addr = start(HttpLimits::default()).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let request = b"GET /peer HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n";
        stream.write_all(request).await.unwrap();
        let response = read_to_close(&mut stream).await;
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("127.0.0.1"), "{response}");

        // A large body arrives whole before the connection closes.
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let request = b"GET /large HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n";
        stream.write_all(request).await.unwrap();
        let response = read_to_close(&mut stream).await;
        assert!(response.len() > 8 << 20, "{}", response.len());
    }

    #[tokio::test]
    async fn slow_headers_and_slow_requests_time_out() {
        let limits = HttpLimits {
            header_timeout: Duration::from_millis(200),
            request_timeout: Duration::from_millis(200),
            ..HttpLimits::default()
        };
        let addr = start(limits).await;
        let mut silent = TcpStream::connect(addr).await.unwrap();
        silent.write_all(b"GET /peer HTTP/1.1\r\n").await.unwrap();
        read_to_close(&mut silent).await;

        let mut slow = TcpStream::connect(addr).await.unwrap();
        let request = b"GET /slow HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n";
        slow.write_all(request).await.unwrap();
        let response = read_to_close(&mut slow).await;
        assert!(response.starts_with("HTTP/1.1 408"), "{response}");
        assert!(response.contains(r#""error":"timeout""#), "{response}");
    }

    #[tokio::test]
    async fn slow_handshakes_time_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let limits = HttpLimits {
            header_timeout: Duration::from_millis(200),
            ..HttpLimits::default()
        };
        // A handshake that never completes.
        let stuck = |_| std::future::pending::<io::Result<TcpStream>>();
        let served = serve_with(listener, stuck, router(), limits, std::future::pending());
        tokio::spawn(served);
        let mut stream = TcpStream::connect(addr).await.unwrap();
        assert_eq!(read_to_close(&mut stream).await, "");
    }

    #[tokio::test]
    async fn connections_over_the_cap_are_closed() {
        let limits = HttpLimits {
            max_connections: 1,
            ..HttpLimits::default()
        };
        let addr = start(limits).await;
        // The first connection holds the only slot once the server has answered on it.
        let mut held = TcpStream::connect(addr).await.unwrap();
        let request = b"GET /peer HTTP/1.1\r\nhost: x\r\n\r\n";
        held.write_all(request).await.unwrap();
        let mut answer = [0; 12];
        held.read_exact(&mut answer).await.unwrap();
        assert_eq!(&answer, b"HTTP/1.1 200");

        let mut refused = TcpStream::connect(addr).await.unwrap();
        assert_eq!(read_to_close(&mut refused).await, "");
    }
}
