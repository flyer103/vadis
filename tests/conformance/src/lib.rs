//! Conformance host crate. Cases are numbered CONF-01…CONF-19 per DESIGN
//! §12.8, living in `tests/conf_<NN>_<slug>.rs`; unimplemented paths all
//! carry `#[ignore = "CONF-NN: depends on <item>"]`.

#![forbid(unsafe_code)]

/// The in-test harness for the R2-2d forwarding cases: a loopback-only
/// mock upstream speaking just enough HTTP/1.1 for `reqwest` to talk to,
/// plus small blocking-HTTP client/tempdir helpers in the CONF-25 style.
///
/// The mock records every request byte-for-byte — the byte-fidelity
/// assertions (CONF-01/02/03/10) compare what the upstream **actually
/// received** against the client's own bytes, never an intermediate
/// structure.
pub mod testkit {
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream as TokioStream};

    // -----------------------------------------------------------------
    // Mock upstream
    // -----------------------------------------------------------------

    /// One request the mock upstream actually received, verbatim.
    #[derive(Debug, Clone)]
    pub struct RecordedRequest {
        pub method: String,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl RecordedRequest {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    /// A canned response; queued in order, the last one repeats.
    #[derive(Clone)]
    pub struct CannedResponse {
        pub status: u16,
        pub reason: &'static str,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl CannedResponse {
        pub fn json(status: u16, reason: &'static str, body: &[u8]) -> Self {
            Self {
                status,
                reason,
                headers: Vec::new(),
                body: body.to_vec(),
            }
        }

        pub fn with_header(mut self, k: &str, v: &str) -> Self {
            self.headers.push((k.to_string(), v.to_string()));
            self
        }
    }

    /// A loopback HTTP upstream. One connection per request is fine: the
    /// router's reqwest client honors `Connection: close`.
    pub struct MockUpstream {
        pub addr: SocketAddr,
        seen: Arc<Mutex<Vec<RecordedRequest>>>,
        queue: Arc<Mutex<VecDeque<CannedResponse>>>,
    }

    impl MockUpstream {
        pub async fn start() -> std::io::Result<Self> {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;
            let seen = Arc::new(Mutex::new(Vec::new()));
            let queue = Arc::new(Mutex::new(VecDeque::new()));
            tokio::spawn(server_loop(listener, Arc::clone(&seen), Arc::clone(&queue)));
            Ok(Self { addr, seen, queue })
        }

        pub fn queue(&self, r: CannedResponse) {
            self.queue.lock().unwrap().push_back(r);
        }

        /// Every request received so far, in order.
        pub fn requests(&self) -> Vec<RecordedRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    async fn server_loop(
        listener: TcpListener,
        seen: Arc<Mutex<Vec<RecordedRequest>>>,
        queue: Arc<Mutex<VecDeque<CannedResponse>>>,
    ) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let seen = Arc::clone(&seen);
            let queue = Arc::clone(&queue);
            tokio::spawn(async move {
                let _ = handle_connection(stream, seen, queue).await;
            });
        }
    }

    async fn handle_connection(
        mut stream: TokioStream,
        seen: Arc<Mutex<Vec<RecordedRequest>>>,
        queue: Arc<Mutex<VecDeque<CannedResponse>>>,
    ) -> std::io::Result<()> {
        let fut = async {
            let req = read_request(&mut stream).await?;
            let canned = {
                let mut q = queue.lock().unwrap();
                if q.len() > 1 {
                    q.pop_front()
                } else {
                    q.front().cloned()
                }
            };
            // An empty queue is a test-authoring bug: answer loudly.
            let canned = canned.unwrap_or_else(|| {
                CannedResponse::json(
                    500,
                    "Internal Server Error",
                    b"{\"mock\":\"no canned response queued\"}",
                )
            });
            seen.lock().unwrap().push(req);
            Ok::<CannedResponse, std::io::Error>(canned)
        };
        let canned = match tokio::time::timeout(Duration::from_secs(30), fut).await {
            Ok(Ok(c)) => c,
            _ => return Ok(()),
        };
        let mut out = format!("HTTP/1.1 {} {}\r\n", canned.status, canned.reason);
        out.push_str("content-type: application/json\r\n");
        for (k, v) in &canned.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str(&format!("content-length: {}\r\n", canned.body.len()));
        out.push_str("connection: close\r\n\r\n");
        stream.write_all(out.as_bytes()).await?;
        stream.write_all(&canned.body).await?;
        stream.shutdown().await?;
        Ok(())
    }

    fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() || haystack.len() < needle.len() {
            return None;
        }
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    async fn read_request(stream: &mut TokioStream) -> std::io::Result<RecordedRequest> {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            if let Some(pos) = find_sub(&buf, b"\r\n\r\n") {
                break pos;
            }
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed before headers completed",
                ));
            }
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_string();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                let v = v.trim();
                if k.eq_ignore_ascii_case("content-length") {
                    content_length = v.parse().unwrap_or(0);
                }
                headers.push((k.to_string(), v.to_string()));
            }
        }
        let body_start = header_end + 4;
        while buf.len() < body_start + content_length {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let body_end = (body_start + content_length).min(buf.len());
        let body = buf[body_start..body_end].to_vec();
        Ok(RecordedRequest {
            method,
            path,
            headers,
            body,
        })
    }

    // -----------------------------------------------------------------
    // Blocking HTTP client (CONF-25 style)
    // -----------------------------------------------------------------

    /// POSTs raw bytes; returns (status, body bytes, response headers).
    pub fn http_post(
        addr: &str,
        path: &str,
        body: &[u8],
        extra_headers: &[(&str, &str)],
    ) -> (u16, Vec<u8>, Vec<(String, String)>) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let mut req = format!("POST {path} HTTP/1.1\r\nHost: {addr}\r\n");
        for (k, v) in extra_headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str(&format!(
            "content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        ));
        stream.write_all(req.as_bytes()).unwrap();
        stream.write_all(body).unwrap();

        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted =>
                {
                    continue;
                }
                Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let text = String::from_utf8_lossy(&buf);
        let (head, body) = match text.split_once("\r\n\r\n") {
            Some((h, b)) => (h.to_string(), b.as_bytes().to_vec()),
            None => (text.into_owned(), Vec::new()),
        };
        let mut head_lines = head.split("\r\n");
        let status: u16 = head_lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .expect("status line");
        let mut headers = Vec::new();
        for line in head_lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        // Trim to the declared content-length when present so the caller
        // compares exactly the response body.
        let declared = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse::<usize>().ok());
        let body = match declared {
            Some(len) if len <= body.len() => body[..len].to_vec(),
            _ => body,
        };
        (status, body, headers)
    }

    // -----------------------------------------------------------------
    // Misc helpers
    // -----------------------------------------------------------------

    pub fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "conf-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A currently-free loopback port (small TOCTOU window, fine for tests).
    pub fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Waits (max ~10s) until the address accepts connections.
    pub fn wait_listening(addr: &str) {
        for _ in 0..200 {
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(50)).is_ok()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("server never came up on {addr}");
    }
}
