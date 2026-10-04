//! Conformance host crate. Cases are numbered CONF-01…CONF-28 per DESIGN
//! §12.8, living in `tests/conf_<NN>_<slug>.rs`; unimplemented paths all
//! carry `#[ignore = "CONF-NN: depends on <item>"]`.

#![forbid(unsafe_code)]

/// The in-test harness for the forwarding cases: a loopback-only
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
    use std::sync::atomic::{AtomicU64, Ordering};
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
        /// `Some(..)` switches the mock into streaming mode: the body is
        /// written with `transfer-encoding: chunked`, one chunk at a time
        /// with the given delay; a chunk flagged `abort` closes the socket
        /// mid-stream (the truncation fixture for CONF-13).
        pub sse_chunks: Option<Vec<SseChunk>>,
    }

    /// One pre-framed streaming chunk.
    #[derive(Clone)]
    pub struct SseChunk {
        pub bytes: Vec<u8>,
        pub delay_ms: u64,
        /// Close the connection instead of finishing the chunked body.
        pub abort: bool,
    }

    impl SseChunk {
        pub fn event(bytes: &[u8]) -> Self {
            Self {
                bytes: bytes.to_vec(),
                delay_ms: 0,
                abort: false,
            }
        }

        pub fn event_after(bytes: &[u8], delay_ms: u64) -> Self {
            Self {
                bytes: bytes.to_vec(),
                delay_ms,
                abort: false,
            }
        }

        /// Close the socket mid-stream after the delay.
        pub fn abort_after(delay_ms: u64) -> Self {
            Self {
                bytes: Vec::new(),
                delay_ms,
                abort: true,
            }
        }
    }

    impl CannedResponse {
        pub fn json(status: u16, reason: &'static str, body: &[u8]) -> Self {
            Self {
                status,
                reason,
                headers: Vec::new(),
                body: body.to_vec(),
                sse_chunks: None,
            }
        }

        /// A 200 SSE response streaming the chunks verbatim.
        pub fn sse(chunks: Vec<SseChunk>) -> Self {
            Self {
                status: 200,
                reason: "OK",
                headers: vec![("content-type".into(), "text/event-stream".into())],
                body: Vec::new(),
                sse_chunks: Some(chunks),
            }
        }

        pub fn with_header(mut self, k: &str, v: &str) -> Self {
            self.headers.push((k.to_string(), v.to_string()));
            self
        }
    }

    /// A loopback HTTP upstream. One connection per request is fine: the
    /// vadis's reqwest client honors `Connection: close`.
    pub struct MockUpstream {
        pub addr: SocketAddr,
        seen: Arc<Mutex<Vec<RecordedRequest>>>,
        queue: Arc<Mutex<VecDeque<CannedResponse>>>,
        /// How many streaming connections ended with a write failure (the
        /// peer — the vadis — closed first): the cancellation signal (DESIGN §12.10.3 R5).
        peer_aborts: Arc<Mutex<u64>>,
    }

    impl MockUpstream {
        pub async fn start() -> std::io::Result<Self> {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;
            let seen = Arc::new(Mutex::new(Vec::new()));
            let queue = Arc::new(Mutex::new(VecDeque::new()));
            let peer_aborts = Arc::new(Mutex::new(0u64));
            tokio::spawn(server_loop(
                listener,
                Arc::clone(&seen),
                Arc::clone(&queue),
                Arc::clone(&peer_aborts),
            ));
            Ok(Self {
                addr,
                seen,
                queue,
                peer_aborts,
            })
        }

        pub fn queue(&self, r: CannedResponse) {
            self.queue.lock().unwrap().push_back(r);
        }

        /// Every request received so far, in order.
        pub fn requests(&self) -> Vec<RecordedRequest> {
            self.seen.lock().unwrap().clone()
        }

        /// Streaming connections whose write side failed because the peer
        /// closed first (the §12.10.3 R5 observable).
        pub fn peer_aborts(&self) -> u64 {
            *self.peer_aborts.lock().unwrap()
        }
    }

    async fn server_loop(
        listener: TcpListener,
        seen: Arc<Mutex<Vec<RecordedRequest>>>,
        queue: Arc<Mutex<VecDeque<CannedResponse>>>,
        peer_aborts: Arc<Mutex<u64>>,
    ) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let seen = Arc::clone(&seen);
            let queue = Arc::clone(&queue);
            let peer_aborts = Arc::clone(&peer_aborts);
            tokio::spawn(async move {
                let _ = handle_connection(stream, seen, queue, peer_aborts).await;
            });
        }
    }

    async fn handle_connection(
        mut stream: TokioStream,
        seen: Arc<Mutex<Vec<RecordedRequest>>>,
        queue: Arc<Mutex<VecDeque<CannedResponse>>>,
        peer_aborts: Arc<Mutex<u64>>,
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
        if canned.sse_chunks.is_some() {
            out.push_str("content-type: text/event-stream\r\n");
            for (k, v) in &canned.headers {
                if !k.eq_ignore_ascii_case("content-type") {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
            }
            out.push_str("transfer-encoding: chunked\r\nconnection: close\r\n\r\n");
            stream.write_all(out.as_bytes()).await?;
            if let Some(chunks) = canned.sse_chunks {
                for ch in &chunks {
                    if ch.delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(ch.delay_ms)).await;
                    }
                    if ch.abort {
                        // Abrupt close: no terminating chunk, no FIN
                        // handshake niceties — the truncation fixture.
                        let _ = stream.shutdown().await;
                        return Ok(());
                    }
                    // Any write on the SSE path failing with the peer gone
                    // — header, body or CRLF — is the same §12.10.3 R5 observable.
                    if stream
                        .write_all(format!("{:x}\r\n", ch.bytes.len()).as_bytes())
                        .await
                        .is_err()
                        || stream.write_all(&ch.bytes).await.is_err()
                        || stream.write_all(b"\r\n").await.is_err()
                    {
                        // The peer (the vadis) closed first: the §12.10.3 R5
                        // cancellation reached the upstream.
                        *peer_aborts.lock().unwrap() += 1;
                        return Ok(());
                    }
                }
            }
            stream.write_all(b"0\r\n\r\n").await?;
            stream.shutdown().await?;
            return Ok(());
        }
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
        http_post_with_opts(addr, path, body, extra_headers, ReadOpts::default())
    }

    /// Read-behavior knobs for `http_post_with_opts`.
    #[derive(Default, Clone, Copy)]
    pub struct ReadOpts {
        /// Stop reading (and drop the connection) after this many body
        /// bytes — the client-disconnect fixture. `None` reads to EOF.
        pub stop_after_body_bytes: Option<usize>,
    }

    /// POSTs raw bytes with explicit read behavior; returns
    /// (status, body bytes read so far, response headers).
    pub fn http_post_with_opts(
        addr: &str,
        path: &str,
        body: &[u8],
        extra_headers: &[(&str, &str)],
        opts: ReadOpts,
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
        let mut head_done = false;
        loop {
            if let Some(stop) = opts.stop_after_body_bytes {
                if head_done && body_len_so_far(&buf) >= stop {
                    // Abrupt client disconnect: drop the socket without a
                    // graceful FIN exchange.
                    drop(stream);
                    break;
                }
            }
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted =>
                {
                    continue
                }
                Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    head_done = buf.contains_subseq(b"\r\n\r\n");
                }
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

    /// Body byte count after dechunking (streaming responses).
    fn body_len_so_far(buf: &[u8]) -> usize {
        match find_sub(buf, b"\r\n\r\n") {
            Some(pos) => dechunk(&buf[pos + 4..]).len(),
            None => 0,
        }
    }

    /// Decodes chunked framing into the raw event byte sequence.
    pub fn dechunk(chunked: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = chunked;
        while let Some(hdr_end) = find_sub(rest, b"\r\n") {
            let hdr = String::from_utf8_lossy(&rest[..hdr_end]);
            let Some(size) = hdr
                .trim()
                .split(';')
                .next()
                .and_then(|s| usize::from_str_radix(s, 16).ok())
            else {
                break;
            };
            if size == 0 {
                break;
            }
            let start = hdr_end + 2;
            if rest.len() < start + size {
                // A chunk cut short by disconnect: keep the prefix.
                out.extend_from_slice(&rest[start..]);
                break;
            }
            out.extend_from_slice(&rest[start..start + size]);
            rest = &rest[start + size..];
            if rest.starts_with(b"\r\n") {
                rest = &rest[2..];
            }
        }
        out
    }

    /// Splits an SSE byte sequence into complete events (blank-line
    /// terminated), returning each event's raw bytes **with** its
    /// terminator — the byte-fidelity unit of CONF-13's assertions.
    pub fn sse_events(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut events = Vec::new();
        let mut rest = bytes;
        while let Some(pos) = find_sub(rest, b"\n\n") {
            // Include one preceding \r of a CRLF terminator.
            let mut end = pos + 2;
            let mut event_end = pos + 1;
            if pos > 0 && rest[pos - 1] == b'\r' {
                event_end = pos;
                end = pos + 2;
            }
            events.push(rest[..end.min(rest.len())].to_vec());
            let _ = event_end;
            rest = &rest[end.min(rest.len())..];
        }
        events
    }

    trait ContainsSub {
        fn contains_subseq(&self, needle: &[u8]) -> bool;
    }

    impl ContainsSub for Vec<u8> {
        fn contains_subseq(&self, needle: &[u8]) -> bool {
            find_sub(self, needle).is_some()
        }
    }

    // -----------------------------------------------------------------
    // Misc helpers
    // -----------------------------------------------------------------

    pub fn tempdir(tag: &str) -> PathBuf {
        // R43-F8: on this platform `SystemTime::as_nanos()` ticks at
        // microsecond resolution, so two arms calling `tempdir` in the
        // same tick were handed ONE directory and one arm's fixtures
        // overwrote the other's (74 call sites — a collision can make a
        // case read another arm's pair and return a false verdict). A
        // process-wide monotonic counter makes the name unique per call;
        // the pid and the caller's tag stay in the name.
        static CALL: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "conf-{}-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            CALL.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write a configuration **pair** (spec §4.14; ADR-037): `root_text`
    /// into `<dir>/config.yaml` and the roster's own text into
    /// `<dir>/providers.yaml`, returning `(root, roster)` — so a case
    /// builds a root+roster pair out of its own fixture text without a
    /// second checked-in fixture (CONF-85).
    pub fn write_config_pair(
        dir: &std::path::Path,
        root_text: &str,
        roster_text: &str,
    ) -> (PathBuf, PathBuf) {
        let root = dir.join("config.yaml");
        let roster = dir.join("providers.yaml");
        std::fs::write(&root, root_text).unwrap();
        std::fs::write(&roster, roster_text).unwrap();
        (root, roster)
    }

    /// A currently-free loopback port. The bind-:0/drop/re-bind sequence
    /// has a TOCTOU window; worse, the OS can hand the SAME port to two
    /// calls in one process (both see it free in the same instant), so two
    /// rigs raced onto one port and the loser connected to the winner's
    /// server — or to nothing once the winner was aborted (R43-F10,
    /// observed as `ConnectionRefused` in conf_85h after
    /// `wait_listening` had succeeded). Two guards close the race:
    ///
    /// - intra-process (R43-7): a process-wide registry never returns a
    ///   port this process has handed out before;
    /// - cross-process (R46-3 — the 2/600 residual R43-7's stress loop
    ///   left open): a per-port lock directory under
    ///   `$TMPDIR/vadis-conf-port-locks/<port>`, claimed with
    ///   `create_dir` (atomic across OS processes) and holding the
    ///   owner's pid. A second process probing the same port finds the
    ///   lock held by a live peer and probes again, so two rig processes
    ///   are never both handed one port — including across *different*
    ///   test binaries, which share the directory.
    ///
    /// The staleness story: a lock outlives its owner by design (there is
    /// no process-exit hook in a test binary), so a claimant finding an
    /// existing lock reads the pid and reaps it only when `kill -0` says
    /// the owner is dead — re-verifying immediately before the atomic
    /// rename that removes it, so a live lock recreated in the gap is not
    /// stolen. A missing/unreadable pid or a recycled pid reads as
    /// *live*: the failure direction is a skipped port (bounded leak,
    /// $TMPDIR is cleaned by the OS), never a double hand-out.
    ///
    /// What this does not close: a process that does not go through
    /// `free_port` (anything outside the suite — and the kernel itself,
    /// handing a locked-but-not-yet-bound port to a peer's probe or a
    /// client connection's source port) can still occupy the port in the
    /// probe→bind gap. That failure is loud (serve's exit 3), never the
    /// silent wrong-server connect of R43-F10, and measured at 2/2400
    /// runs in R46-3's 6-way loop (the F10 class itself: 0/12000
    /// cross-process allocations, 0 duplicates).
    pub fn free_port() -> u16 {
        static HANDED_OUT: std::sync::OnceLock<Mutex<std::collections::HashSet<u16>>> =
            std::sync::OnceLock::new();
        let lock_root = std::env::temp_dir().join("vadis-conf-port-locks");
        std::fs::create_dir_all(&lock_root).unwrap();
        let mut missed = 0u32;
        loop {
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            if !HANDED_OUT
                .get_or_init(|| Mutex::new(std::collections::HashSet::new()))
                .lock()
                .unwrap()
                .insert(port)
            {
                continue;
            }
            if claim_port_across_processes(&lock_root, port) {
                return port;
            }
            // A live peer process owns this port: forget it locally and
            // probe for another. A bounded spin: if every ephemeral port
            // is held by live peers (legitimate saturation), fail loudly
            // rather than spin forever.
            HANDED_OUT
                .get_or_init(|| Mutex::new(std::collections::HashSet::new()))
                .lock()
                .unwrap()
                .remove(&port);
            missed += 1;
            assert!(
                missed < 4096,
                "free_port: {missed} consecutive probes found every offered port \
                 locked in {lock_root:?} by a live peer — the ephemeral range is \
                 saturated; refusing to spin"
            );
        }
    }

    /// Claim `port` for this process across OS processes. Returns true when
    /// this process owns the lock directory on return.
    fn claim_port_across_processes(lock_root: &std::path::Path, port: u16) -> bool {
        let lockdir = lock_root.join(port.to_string());
        // A lock is a directory. Anything else at the path is debris of a
        // foreign or superseded scheme (it can never carry a live owner):
        // remove it so the range cannot be bricked by stale format files.
        if lockdir.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
            let _ = std::fs::remove_file(&lockdir);
        }
        for attempt in 0..2 {
            // create_dir is the atomic cross-process claim: exactly one
            // claimant wins it for a given path.
            match std::fs::create_dir(&lockdir) {
                Ok(()) => {
                    std::fs::write(lockdir.join("pid"), std::process::id().to_string()).unwrap();
                    return true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && !lock_owner_is_alive(&lockdir) {
                        reap_stale_lock(&lockdir);
                        continue; // one retry: whoever's create_dir wins owns it
                    }
                    return false; // a live peer (or a peer that just beat us) owns it
                }
                Err(e) => panic!("free_port: cannot claim the port lock {lockdir:?}: {e}"),
            }
        }
        false
    }

    /// Is the process recorded in the lock's `pid` file still running?
    /// Missing, unreadable or unparsable content reads as *live*
    /// (conservative: the owner may be mid-claim; a wrong "dead" verdict
    /// is the only direction that can double-hand a port).
    fn lock_owner_is_alive(lockdir: &std::path::Path) -> bool {
        let Ok(text) = std::fs::read_to_string(lockdir.join("pid")) else {
            return true;
        };
        let Ok(pid) = text.trim().parse::<u32>() else {
            return true;
        };
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(true)
    }

    /// Remove a lock whose owner was just verified dead. The rename is
    /// atomic and names the exact directory we checked; the owner's death
    /// is re-verified immediately before it, so a lock recreated by a new
    /// owner in the gap reads as alive and is left alone.
    fn reap_stale_lock(lockdir: &std::path::Path) {
        if lock_owner_is_alive(lockdir) {
            return;
        }
        let graveyard = lockdir.with_file_name(format!(
            "{}.stale-{}-{}",
            lockdir.file_name().unwrap().to_string_lossy(),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        if std::fs::rename(lockdir, &graveyard).is_ok() {
            let _ = std::fs::remove_dir_all(&graveyard);
        }
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

    // -----------------------------------------------------------------
    // The plan-first rig (CONF-32…, spec §4.6 / ADR-014): two mock
    // upstreams — one `account: coding_plan`, one `account: api` serving
    // the same model id — plus the real `serve` assembly, so every
    // account-state assertion is made against the requests the mocks
    // actually received, the store's event rows and the trace.
    // -----------------------------------------------------------------

    /// The canned 403 that moves the account: the wording contains
    /// `insufficient_quota`, one of the classifier's QUOTA_EXHAUSTED_PATTERNS
    /// — anything else classifies as `auth` and moves nothing. The
    /// `Retry-After: 1` header shrinks the ADR-011 provider demotion that
    /// follows the 403 from the 60s default to one second, so probe cases
    /// run without sleeping a minute (the demotion is route availability,
    /// deliberately independent of the family's own cooldown knob).
    pub fn plan_forbidden_403() -> CannedResponse {
        CannedResponse::json(
            403,
            "Forbidden",
            br#"{"error":{"message":"You have exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
        )
        .with_header("retry-after", "1")
    }

    /// A 200 chat completion with a fixed usage the cost/quota paths can
    /// reason about: 100 prompt tokens (20 cached) + 5 completion.
    pub fn plan_ok(content: &str) -> CannedResponse {
        let body = format!(
            r#"{{"id":"ok","choices":[{{"index":0,"message":{{"role":"assistant","content":"{content}"}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{{"cached_tokens":20}}}}}}"#
        );
        CannedResponse::json(200, "OK", body.as_bytes())
    }

    /// The plan-first config: family `m1` on `p-plan` (coding_plan) with
    /// `p-api` (api) as the spill. `quota_yaml` is inserted into the
    /// p-plan provider (pass "" for a plan whose allowance is
    /// unpublished — GAP-Q1's legal case); `policy_yaml` replaces the
    /// plan_policy body (family/primary/overflow are always required).
    pub fn plan_config_yaml(
        plan_port: u16,
        api_port: u16,
        listen_port: u16,
        quota_yaml: &str,
        policy_yaml: &str,
    ) -> String {
        format!(
            r#"server:   {{ addr: "127.0.0.1:{listen_port}", upstream_attempt_timeout: 10s, request_timeout: 30s }}
session:  {{ key_sources: ["prompt_cache_key"], ttl: 11h }}
cache:    {{ sticky: true, breakeven: {{ enabled: true, min_remaining_turns: 2, safety_factor: 1.1 }} }}
trace:    {{ dir: "./state/traces", rollover: hourly }}

providers:
  - name: p-plan
    urls:
      chat: http://127.0.0.1:{plan_port}/v1/chat/completions
    api_key_env: CONF_PF_PLAN_KEY
    wire_api: chat
    supports: [chat]
    account: coding_plan{quota_yaml}
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.001
          input_hit: 0.0001
          cache_write: 0.0
          output: 0.002
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"
  - name: p-api
    urls:
      chat: http://127.0.0.1:{api_port}/v1/chat/completions
    api_key_env: CONF_PF_API_KEY
    wire_api: chat
    supports: [chat]
    account: api
    models:
      - id: m1
        context: 128k
        price:
          input_miss: 0.002
          input_hit: 0.0002
          cache_write: 0.0
          output: 0.004
          peak: {{ multiplier: 1.0, windows: [] }}
        source: "mock upstream (no price; test fixture)"

aliases: {{}}
plugins: []
fallback: []

plan_policy:
{policy_yaml}
"#
        )
    }

    /// The default policy body: spill, probe recovery, `cooldown: 0s`.
    /// The zero cooldown is deliberate (the operator's probe convention):
    /// it removes "the cooldown has not elapsed" as an alternative
    /// explanation wherever a request must NOT probe, making those
    /// assertions decisive rather than incidental.
    pub const PLAN_POLICY_DEFAULT: &str = "  family: m1\n  primary: p-plan/m1\n  overflow: p-api/m1\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 0s";

    /// A running plan-first rig. `vadis_cli` is a dev-dependency (the
    /// serve spawn stays in the test files, the CONF-25 precedent), so
    /// this is assembled from [`plan_rig_parts`] there.
    pub struct PlanRig {
        pub plan: MockUpstream,
        pub api: MockUpstream,
        pub listen_addr: String,
        pub dir: PathBuf,
        pub serve_task: tokio::task::JoinHandle<i32>,
    }

    /// The parts of a plan-first rig: the two mocks, the tempdir (with
    /// `config.yaml` written) and the free listen address. The caller
    /// spawns the real serve assembly and builds a [`PlanRig`].
    pub async fn plan_rig_parts(
        tag: &str,
        quota_yaml: &str,
        policy_yaml: &str,
    ) -> (MockUpstream, MockUpstream, PathBuf, String) {
        let dir = tempdir(tag);
        let plan = MockUpstream::start().await.unwrap();
        let api = MockUpstream::start().await.unwrap();
        let listen_port = free_port();
        let listen_addr = format!("127.0.0.1:{listen_port}");
        let config_path = dir.join("config.yaml");
        std::fs::write(
            &config_path,
            plan_config_yaml(
                plan.addr.port(),
                api.addr.port(),
                listen_port,
                quota_yaml,
                policy_yaml,
            ),
        )
        .unwrap();
        std::env::set_var("CONF_PF_PLAN_KEY", "sk-plan");
        std::env::set_var("CONF_PF_API_KEY", "sk-api");
        (plan, api, dir, listen_addr)
    }

    impl PlanRig {
        /// One buffered chat request for the family's primary route,
        /// with or without a session key. Returns (status, body, headers).
        pub fn post(
            &self,
            session: Option<&str>,
            turn: u32,
        ) -> (u16, Vec<u8>, Vec<(String, String)>) {
            let key = session
                .map(|s| format!(r#","prompt_cache_key":"{s}""#))
                .unwrap_or_default();
            let body = format!(
                r#"{{"model":"p-plan/m1","messages":[{{"role":"user","content":"turn {turn}"}}]{key},"stream":false}}"#
            );
            http_post(
                &self.listen_addr,
                "/v1/chat/completions",
                body.as_bytes(),
                &[],
            )
        }

        /// Aborts serve. The state directory survives for the reads.
        pub fn stop(self) -> std::path::PathBuf {
            let dir = self.dir.clone();
            self.serve_task.abort();
            drop(self.serve_task);
            dir
        }
    }
}
