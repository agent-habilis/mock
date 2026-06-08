use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::rt::{Read, Write};
use hyper::{Request, Uri};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;

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
///
/// Built on hyper's connection-level client (`hyper::client::conn::http1`) — no
/// pooling — so a fresh connection is dialed per attempt: plaintext TCP for
/// `http://`, or a rustls TLS session (using `tls_config`) for `https://`.
pub(crate) async fn proxy_request(
    tls_config: &Arc<ClientConfig>,
    origin: &str,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
    retries: u32,
    overwrite_request_headers: &HashMap<String, serde_json::Value>,
    proxy_url: &str,
) -> Result<ProxyResponse, MockerError> {
    let full_url = format!("{origin}{url}");
    let body = Bytes::from(body);

    if retries == 0 {
        return send_once(
            tls_config,
            &full_url,
            method,
            headers,
            body,
            overwrite_request_headers,
            proxy_url,
        )
        .await;
    }

    let tls_config = tls_config.clone();
    let method = method.to_string();
    let headers = headers.to_vec();
    let overwrite = overwrite_request_headers.clone();
    let proxy_url = proxy_url.to_string();
    retry(
        move || {
            let tls_config = tls_config.clone();
            let full_url = full_url.clone();
            let method = method.clone();
            let headers = headers.clone();
            let body = body.clone(); // `Bytes` clones are O(1) (refcounted).
            let overwrite = overwrite.clone();
            let proxy_url = proxy_url.clone();
            async move {
                send_once(
                    &tls_config,
                    &full_url,
                    &method,
                    &headers,
                    body,
                    &overwrite,
                    &proxy_url,
                )
                .await
            }
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

/// Dial the origin (or upstream proxy), send one request, and read the full
/// response. Bounded by [`PROXY_TIMEOUT`].
async fn send_once(
    tls_config: &Arc<ClientConfig>,
    full_url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Bytes,
    overwrite_request_headers: &HashMap<String, serde_json::Value>,
    proxy_url: &str,
) -> Result<ProxyResponse, MockerError> {
    let target: Uri = full_url
        .parse()
        .map_err(|e: hyper::http::uri::InvalidUri| MockerError::HttpError(e.to_string()))?;
    let scheme = target.scheme_str().unwrap_or("http");
    let is_https = scheme.eq_ignore_ascii_case("https");
    let host = target
        .host()
        .ok_or_else(|| MockerError::HttpError(format!("origin URL has no host: {full_url}")))?
        .to_string();
    // The Host header the legacy client used to synthesize from the URI authority.
    let authority = target.authority().map(|a| a.as_str().to_string());
    let port = target.port_u16().unwrap_or(if is_https { 443 } else { 80 });

    // Pick the dial target and the request-target form. hyper's h1 client
    // serializes the request `Uri` verbatim, so origin-form (path only) goes
    // direct to origin and absolute-form (full URL) goes to a proxy (RFC 9112
    // §3.2.1/§3.2.2). A proxy is always dialed in plaintext.
    let (dial_host, dial_port, use_tls, req_uri) = if proxy_url.is_empty() {
        let path_and_query = target.path_and_query().map_or("/", |p| p.as_str());
        let req_uri: Uri = path_and_query
            .parse()
            .map_err(|e: hyper::http::uri::InvalidUri| MockerError::HttpError(e.to_string()))?;
        (host.clone(), port, is_https, req_uri)
    } else {
        let (proxy_host, proxy_port) = parse_proxy_authority(proxy_url)?;
        let req_uri: Uri = full_url
            .parse()
            .map_err(|e: hyper::http::uri::InvalidUri| MockerError::HttpError(e.to_string()))?;
        (proxy_host, proxy_port, false, req_uri)
    };

    let req = build_request(
        method,
        req_uri,
        headers,
        overwrite_request_headers,
        authority.as_deref(),
        body,
    )?;

    let send = async move {
        let tcp = TcpStream::connect((dial_host.as_str(), dial_port))
            .await
            .map_err(|e| MockerError::HttpError(format!("failed to connect to origin: {e}")))?;

        if use_tls {
            let connector = TlsConnector::from(tls_config.clone());
            let server_name = ServerName::try_from(host.as_str())
                .map_err(|e| MockerError::HttpError(format!("invalid TLS server name: {e}")))?
                .to_owned();
            let tls = connector
                .connect(server_name, tcp)
                .await
                .map_err(|e| MockerError::HttpError(format!("TLS handshake failed: {e}")))?;
            send_over(TokioIo::new(tls), req).await
        } else {
            send_over(TokioIo::new(tcp), req).await
        }
    };

    tokio::time::timeout(PROXY_TIMEOUT, send)
        .await
        .map_err(|_| MockerError::HttpError("proxy request timed out".to_string()))?
}

/// Handshake over an established (TLS or plaintext) stream, send the request,
/// and collect the response.
async fn send_over<IO>(io: IO, req: Request<Full<Bytes>>) -> Result<ProxyResponse, MockerError>
where
    IO: Read + Write + Unpin + Send + 'static,
{
    let (mut sender, conn) = http1::handshake(io)
        .await
        .map_err(|e| MockerError::HttpError(e.to_string()))?;

    // The connection must be driven concurrently while the request is in flight.
    let conn_task = tokio::spawn(async move {
        let _ = conn.await;
    });

    let resp = sender
        .send_request(req)
        .await
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

    conn_task.abort();

    Ok(ProxyResponse {
        status,
        headers: resp_headers,
        body: resp_body,
    })
}

/// Build the outgoing request: method + request-target `Uri`, forwarded headers
/// (minus hop-by-hop), then overwrite headers. If neither supplies a `Host`, one
/// is synthesized from the origin authority — hyper's h1 client requires a Host
/// header and the old pooled client added it implicitly.
fn build_request(
    method: &str,
    uri: Uri,
    headers: &[(String, String)],
    overwrite_request_headers: &HashMap<String, serde_json::Value>,
    origin_authority: Option<&str>,
    body: Bytes,
) -> Result<Request<Full<Bytes>>, MockerError> {
    let hyper_method = hyper::Method::from_bytes(method.as_bytes())
        .map_err(|e| MockerError::HttpError(e.to_string()))?;

    let mut builder = Request::builder().method(hyper_method).uri(uri);

    let mut has_host = false;

    // Drop hop-by-hop + content-length before forwarding to origin; hyper
    // re-frames the body from the owned `Full<Bytes>` and sets content-length.
    let forwarded = crate::http::headers::strip_hop_by_hop(headers);
    for (key, value) in &forwarded {
        if key.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        builder = builder.header(key.as_str(), value.as_str());
    }

    // Apply overwrite headers (these override originals).
    for (key, value) in overwrite_request_headers {
        if key.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        let val_str = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        builder = builder.header(key.as_str(), val_str.as_str());
    }

    if !has_host && let Some(authority) = origin_authority {
        builder = builder.header("host", authority);
    }

    builder
        .body(Full::new(body))
        .map_err(|e| MockerError::HttpError(e.to_string()))
}

/// Parse the `(host, port)` to dial from a proxy URL, defaulting to port 80.
fn parse_proxy_authority(proxy_url: &str) -> Result<(String, u16), MockerError> {
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
    if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port.parse::<u16>().map_err(|_| {
            MockerError::HttpError(format!("invalid proxy port in URL: {proxy_url}"))
        })?;
        Ok((host.to_string(), port))
    } else {
        Ok((authority.to_string(), 80))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::Response;
    use hyper::server::conn::http1 as server_http1;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    /// A TLS config for the tests. Only plaintext paths are exercised, so its
    /// roots don't matter — it just satisfies the `proxy_request` signature.
    fn tls_config() -> Arc<ClientConfig> {
        crate::server::build_tls_config()
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

        let result = proxy_request(
            &tls_config(),
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

        let result = proxy_request(
            &tls_config(),
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
        assert_eq!(
            parse_proxy_authority("http://gw:8080").unwrap(),
            ("gw".to_string(), 8080)
        );
        assert_eq!(
            parse_proxy_authority("http://gw").unwrap(),
            ("gw".to_string(), 80)
        );
        assert_eq!(
            parse_proxy_authority("https://gw:3128/path").unwrap(),
            ("gw".to_string(), 3128)
        );
        assert!(parse_proxy_authority("http://").is_err());
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
                let _ = server_http1::Builder::new()
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

        let result = proxy_request(
            &tls_config(),
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
                let _ = server_http1::Builder::new()
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

        let result = proxy_request(
            &tls_config(),
            &origin,
            "GET",
            "/",
            &[],
            vec![],
            0,
            &overwrite,
            "",
        )
        .await;

        // The request should succeed
        assert!(result.is_ok());
        drop(handle);
    }

    #[tokio::test]
    async fn test_proxy_request_connection_refused() {
        let result = proxy_request(
            &tls_config(),
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
        let result = proxy_request(
            &tls_config(),
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

        let result = proxy_request(
            &tls_config(),
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

        let result = proxy_request(
            &tls_config(),
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
