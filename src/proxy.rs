use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper::{Request, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connect, Connected, Connection, HttpConnector};
use tokio::net::TcpStream;

use crate::error::MockerError;
use crate::util::backoff::Backoff;
use crate::util::retry::retry;

const PROXY_TIMEOUT: Duration = Duration::from_secs(30);

/// The result of proxying a request to the origin server.
pub(crate) struct ProxyResponse {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

/// Forward a request to origin, optionally through an upstream HTTP proxy and
/// with retries.
pub(crate) async fn proxy_request(
    http_client: &Client<HttpConnector, Full<Bytes>>,
    origin: &str,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
    retries: u32,
    overwrite_request_headers: &HashMap<String, serde_json::Value>,
    proxy_url: &str,
) -> Result<ProxyResponse, MockerError> {
    // The request-target is always the absolute origin URL. On a direct
    // connection hyper sends it origin-form; through `ProxyConnector` (which
    // reports a proxied connection) hyper rewrites it to absolute-form
    // (RFC 9112 §3.2.2) so the proxy learns which origin to forward to.
    let full_url = format!("{origin}{url}");
    let body = Bytes::from(body);

    if proxy_url.is_empty() {
        send_with_retries(
            http_client,
            &full_url,
            method,
            headers,
            body,
            retries,
            overwrite_request_headers,
        )
        .await
    } else {
        let authority = proxy_authority(proxy_url)?;
        let proxy_client = Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(ProxyConnector::new(authority));
        send_with_retries(
            &proxy_client,
            &full_url,
            method,
            headers,
            body,
            retries,
            overwrite_request_headers,
        )
        .await
    }
}

/// Run `do_request`, retrying transient failures (network errors and 5xx) up to
/// `retries` times with exponential backoff.
async fn send_with_retries<C>(
    client: &Client<C, Full<Bytes>>,
    full_url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Bytes,
    retries: u32,
    overwrite_request_headers: &HashMap<String, serde_json::Value>,
) -> Result<ProxyResponse, MockerError>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    if retries == 0 {
        return do_request(
            client,
            full_url,
            method,
            headers,
            body,
            overwrite_request_headers,
        )
        .await;
    }

    let full_url = full_url.to_string();
    let method = method.to_string();
    let headers = headers.to_vec();
    let overwrite = overwrite_request_headers.clone();
    let client = client.clone();
    retry(
        move || {
            let full_url = full_url.clone();
            let method = method.clone();
            let headers = headers.clone();
            let body = body.clone(); // `Bytes` clones are O(1) (refcounted).
            let overwrite = overwrite.clone();
            let client = client.clone();
            async move { do_request(&client, &full_url, &method, &headers, body, &overwrite).await }
        },
        retries,
        // Retry transient origin failures: network errors and 5xx.
        |result| match result {
            Err(_) => true,
            Ok(resp) => resp.status >= 500,
        },
        Backoff::new(1000, 30000),
    )
    .await
}

async fn do_request<C>(
    client: &Client<C, Full<Bytes>>,
    full_url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Bytes,
    overwrite_request_headers: &HashMap<String, serde_json::Value>,
) -> Result<ProxyResponse, MockerError>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    let uri: Uri = full_url
        .parse()
        .map_err(|e: hyper::http::uri::InvalidUri| MockerError::HttpError(e.to_string()))?;

    let hyper_method = hyper::Method::from_bytes(method.as_bytes())
        .map_err(|e| MockerError::HttpError(e.to_string()))?;

    let mut builder = Request::builder().method(hyper_method).uri(uri);

    // Drop hop-by-hop + content-length before forwarding to origin; hyper
    // re-frames the body from the owned `Full<Bytes>` and sets content-length.
    let forwarded = crate::http::headers::strip_hop_by_hop(headers);
    for (key, value) in &forwarded {
        builder = builder.header(key.as_str(), value.as_str());
    }

    // Apply overwrite headers (these override originals)
    for (key, value) in overwrite_request_headers {
        let val_str = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        builder = builder.header(key.as_str(), val_str.as_str());
    }

    let req = builder
        .body(Full::new(body))
        .map_err(|e| MockerError::HttpError(e.to_string()))?;

    let resp = tokio::time::timeout(PROXY_TIMEOUT, client.request(req))
        .await
        .map_err(|_| MockerError::HttpError("proxy request timed out".to_string()))?
        .map_err(|e| MockerError::HttpError(e.to_string()))?;

    let status = resp.status().as_u16();

    let raw_resp_headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    // Strip hop-by-hop + content-length so re-emission to the client doesn't
    // conflict with hyper's own framing of the collected body. content-encoding
    // is preserved here because the mock-write path needs it to decompress.
    let resp_headers = crate::http::headers::strip_hop_by_hop(&raw_resp_headers);

    let resp_body = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| MockerError::HttpError(e.to_string()))?
        .to_bytes()
        .to_vec();

    Ok(ProxyResponse {
        status,
        headers: resp_headers,
        body: resp_body,
    })
}

/// Parse the `host:port` authority from a proxy URL, defaulting to port 80.
fn proxy_authority(proxy_url: &str) -> Result<String, MockerError> {
    let without_scheme = proxy_url
        .strip_prefix("http://")
        .or_else(|| proxy_url.strip_prefix("https://"))
        .unwrap_or(proxy_url);
    let authority = without_scheme.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return Err(MockerError::HttpError(format!(
            "invalid proxy URL: {proxy_url}"
        )));
    }
    if authority.contains(':') {
        Ok(authority.to_string())
    } else {
        Ok(format!("{authority}:80"))
    }
}

/// A connector that always dials a fixed upstream HTTP proxy and reports the
/// connection as proxied, so hyper emits absolute-form request-targets
/// (RFC 9112 §3.2.2) the proxy can forward to the origin.
#[derive(Clone)]
struct ProxyConnector {
    authority: Arc<str>,
}

impl ProxyConnector {
    fn new(authority: String) -> Self {
        Self {
            authority: Arc::from(authority),
        }
    }
}

impl tower_service::Service<Uri> for ProxyConnector {
    type Response = ProxyStream;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<ProxyStream, std::io::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let authority = self.authority.clone();
        Box::pin(async move {
            let stream = TcpStream::connect(&*authority).await?;
            Ok(ProxyStream(hyper_util::rt::TokioIo::new(stream)))
        })
    }
}

/// IO wrapper that tags the underlying socket as a proxied connection.
struct ProxyStream(hyper_util::rt::TokioIo<TcpStream>);

impl Connection for ProxyStream {
    fn connected(&self) -> Connected {
        Connected::new().proxy(true)
    }
}

impl Read for ProxyStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl Write for ProxyStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::Response;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    fn make_client() -> Client<HttpConnector, Full<Bytes>> {
        Client::builder(TokioExecutor::new()).build_http()
    }

    /// Accept one connection, capture the raw request head (request line +
    /// headers), reply `200 OK`, and return the captured text via the task.
    async fn start_capture_server() -> (SocketAddr, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                .await;
            let _ = stream.flush().await;
            String::from_utf8_lossy(&buf).into_owned()
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn proxy_request_uses_absolute_form_request_target() {
        // RFC 9112 §3.2.2: when sending to a proxy (other than CONNECT), the
        // request-target MUST be in absolute-form (the full origin URI).
        let (proxy_addr, handle) = start_capture_server().await;
        let client = make_client();

        let result = proxy_request(
            &client,
            "http://origin.example.com",
            "GET",
            "/foo?bar=1",
            &[],
            vec![],
            0,
            &HashMap::new(),
            &format!("http://{proxy_addr}"),
        )
        .await
        .unwrap();
        assert_eq!(result.status, 200);

        let captured = handle.await.unwrap();
        let request_line = captured.lines().next().unwrap_or_default();
        assert_eq!(
            request_line, "GET http://origin.example.com/foo?bar=1 HTTP/1.1",
            "proxied request must use absolute-form (RFC 9112 §3.2.2); got: {request_line}"
        );
        // The Host header still names the origin, not the proxy.
        assert!(
            captured
                .to_ascii_lowercase()
                .contains("host: origin.example.com"),
            "captured head:\n{captured}"
        );
    }

    #[tokio::test]
    async fn direct_request_uses_origin_form_request_target() {
        // RFC 9112 §3.2.1: a direct request to the origin uses origin-form
        // (the absolute path + query only).
        let (origin_addr, handle) = start_capture_server().await;
        let client = make_client();

        let result = proxy_request(
            &client,
            &format!("http://{origin_addr}"),
            "GET",
            "/foo",
            &[],
            vec![],
            0,
            &HashMap::new(),
            "",
        )
        .await
        .unwrap();
        assert_eq!(result.status, 200);

        let captured = handle.await.unwrap();
        let request_line = captured.lines().next().unwrap_or_default();
        assert_eq!(
            request_line, "GET /foo HTTP/1.1",
            "direct request must use origin-form (RFC 9112 §3.2.1); got: {request_line}"
        );
    }

    #[test]
    fn proxy_authority_defaults_and_parsing() {
        assert_eq!(proxy_authority("http://gw:8080").unwrap(), "gw:8080");
        assert_eq!(proxy_authority("http://gw").unwrap(), "gw:80");
        assert_eq!(proxy_authority("https://gw:3128/path").unwrap(), "gw:3128");
        assert!(proxy_authority("http://").is_err());
    }

    async fn start_test_server(
        status: u16,
        body: &'static str,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            // Accept just one connection for the test
            if let Ok((stream, _)) = listener.accept().await {
                let io = TokioIo::new(stream);
                let _ = http1::Builder::new()
                    .serve_connection(
                        io,
                        service_fn(move |_req: Request<hyper::body::Incoming>| {
                            let body = body.to_string();
                            async move {
                                let resp = Response::builder()
                                    .status(status)
                                    .header("x-test", "hello")
                                    .body(Full::new(Bytes::from(body)))
                                    .unwrap();
                                Ok::<_, hyper::Error>(resp)
                            }
                        }),
                    )
                    .await;
            }
        });

        (addr, handle)
    }

    #[tokio::test]
    async fn test_proxy_request_basic() {
        let (addr, _handle) = start_test_server(200, "ok").await;
        let origin = format!("http://{addr}");
        let client = make_client();

        let result = proxy_request(
            &client,
            &origin,
            "GET",
            "/test",
            &[],
            vec![],
            0,
            &HashMap::new(),
            "",
        )
        .await
        .unwrap();

        assert_eq!(result.status, 200);
        assert_eq!(result.body, b"ok");
        assert!(
            result
                .headers
                .iter()
                .any(|(k, v)| k == "x-test" && v == "hello")
        );
    }

    #[tokio::test]
    async fn test_proxy_request_with_overwrite_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let io = TokioIo::new(stream);
                let _ = http1::Builder::new()
                    .serve_connection(
                        io,
                        service_fn(|req: Request<hyper::body::Incoming>| async move {
                            let host = req
                                .headers()
                                .get("host")
                                .map(|v| v.to_str().unwrap_or("").to_string())
                                .unwrap_or_default();
                            let resp = Response::builder()
                                .status(200)
                                .body(Full::new(Bytes::from(host)))
                                .unwrap();
                            Ok::<_, hyper::Error>(resp)
                        }),
                    )
                    .await;
            }
        });

        let origin = format!("http://{addr}");
        let mut overwrite = HashMap::new();
        overwrite.insert(
            "host".to_string(),
            serde_json::Value::String("custom-host.example.com".to_string()),
        );

        let client = make_client();
        let result =
            proxy_request(&client, &origin, "GET", "/", &[], vec![], 0, &overwrite, "").await;

        // The request should succeed
        assert!(result.is_ok());
        drop(handle);
    }

    #[tokio::test]
    async fn test_proxy_request_connection_refused() {
        let client = make_client();
        let result = proxy_request(
            &client,
            "http://127.0.0.1:1",
            "GET",
            "/test",
            &[],
            vec![],
            0,
            &HashMap::new(),
            "",
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_proxy_request_invalid_method() {
        let client = make_client();
        let result = proxy_request(
            &client,
            "http://127.0.0.1:1",
            "INVALID METHOD WITH SPACES",
            "/test",
            &[],
            vec![],
            0,
            &HashMap::new(),
            "",
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_proxy_response_fields() {
        let (addr, _handle) = start_test_server(201, "created").await;
        let origin = format!("http://{addr}");
        let client = make_client();

        let result = proxy_request(
            &client,
            &origin,
            "POST",
            "/resource",
            &[],
            vec![],
            0,
            &HashMap::new(),
            "",
        )
        .await
        .unwrap();

        assert_eq!(result.status, 201);
        assert_eq!(result.body, b"created");
    }

    #[tokio::test]
    async fn test_proxy_request_with_proxy_url() {
        let (addr, _handle) = start_test_server(200, "proxied").await;
        let proxy_url = format!("http://{addr}");
        let client = make_client();

        let result = proxy_request(
            &client,
            "http://original-host.example.com",
            "GET",
            "/path",
            &[],
            vec![],
            0,
            &HashMap::new(),
            &proxy_url,
        )
        .await
        .unwrap();

        assert_eq!(result.status, 200);
        assert_eq!(result.body, b"proxied");
    }
}
