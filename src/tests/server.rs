//! Minimal hand-rolled HTTP test server used by the tests.
//!
//! It avoids any extra dependency: a `std::net::TcpListener` serves one
//! thread per connection and delegates every request to the handler
//! closure. Concurrency is measured while the *handler* runs only, which
//! makes the bounded-concurrency assertions deterministic (socket teardown
//! no longer counts as activity).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Parsed subset of a client request handed to [`TestServer`] handlers.
#[derive(Debug, Clone)]
pub struct TestRequest {
    /// Request path (e.g. `/app/foo`).
    pub path: String,
    /// Lowercased header names mapped to their values.
    pub headers: HashMap<String, String>,
}

/// Response builder handed back by handlers.
#[derive(Debug, Clone)]
pub struct TestResponse {
    status: u16,
    body: Vec<u8>,
    content_type: String,
    extra_headers: Vec<(String, String)>,
    content_length_override: Option<usize>,
    chunked: bool,
}

impl TestResponse {
    /// HTTP 200 response with the given body.
    pub fn ok(body: impl Into<String>) -> Self {
        Self::status(200, body)
    }

    /// Response with a custom status code and body.
    pub fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into().into_bytes(),
            content_type: "text/html".to_owned(),
            extra_headers: Vec::new(),
            content_length_override: None,
            chunked: false,
        }
    }

    /// Raw bytes body (for charset tests).
    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        let mut response = Self::status(status, "");
        response.body = body;
        response.content_type = content_type.to_owned();
        response
    }

    /// Response with a custom `Content-Type` header.
    pub fn with_content_type(mut self, content_type: &str) -> Self {
        self.content_type = content_type.to_owned();
        self
    }

    /// Adds a response header.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.extra_headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// HTTP redirect (302) with a `Location` header.
    pub fn redirect(location: &str) -> Self {
        Self::status(302, "")
            .with_header("Location", location)
            .with_content_type("text/html")
    }

    /// HTTP 429 with a `Retry-After` header (seconds form).
    pub fn retry_after(seconds: u64) -> Self {
        Self::status(429, "slow down").with_header("Retry-After", &seconds.to_string())
    }

    /// Lies about the `Content-Length` (larger than the real body) so the
    /// client body read fails mid-flight.
    pub fn truncated_body(body: impl Into<String>) -> Self {
        let body = body.into();
        let mut response = Self::ok(body.clone());
        response.content_length_override = Some(body.len() + 50);
        response
    }

    /// Sends the body with `Transfer-Encoding: chunked`, two bytes per
    /// chunk, forcing the client through many small reads.
    pub fn chunked(body: impl Into<String>) -> Self {
        let mut response = Self::ok(body);
        response.chunked = true;
        response
    }

    fn write_to(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        let mut extra = String::new();
        for (name, value) in &self.extra_headers {
            extra.push_str(name);
            extra.push_str(": ");
            extra.push_str(value);
            extra.push_str("\r\n");
        }
        if self.chunked {
            extra.push_str("Transfer-Encoding: chunked\r\n");
            let head = format!(
                "HTTP/1.1 {} X\r\nContent-Type: {}\r\n{extra}Connection: close\r\n\r\n",
                self.status, self.content_type
            );
            stream.write_all(head.as_bytes())?;
            for pair in self.body.chunks(2) {
                write!(stream, "{:x}\r\n", pair.len())?;
                stream.write_all(pair)?;
                stream.write_all(b"\r\n")?;
            }
            stream.write_all(b"0\r\n\r\n")?;
        } else {
            let length = self.content_length_override.unwrap_or(self.body.len());
            let head = format!(
                "HTTP/1.1 {} X\r\nContent-Type: {}\r\n{extra}Content-Length: {length}\r\nConnection: close\r\n\r\n",
                self.status, self.content_type
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(&self.body)?;
        }
        stream.flush()
    }
}

type Handler = dyn Fn(&TestRequest) -> TestResponse + Send + Sync;

/// A running test server. Dropping it stops the listener; in-flight handler
/// threads may linger briefly without impacting the tests.
pub struct TestServer {
    addr: SocketAddr,
    hit_count: Arc<AtomicUsize>,
    concurrent_max: Arc<AtomicUsize>,
    seen_headers: Arc<Mutex<HashMap<String, String>>>,
}

impl TestServer {
    /// Starts a server whose every request is answered by `handler`.
    pub fn spawn(handler: impl Fn(&TestRequest) -> TestResponse + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let hit_count = Arc::new(AtomicUsize::new(0));
        let active_count = Arc::new(AtomicUsize::new(0));
        let concurrent_max = Arc::new(AtomicUsize::new(0));
        let seen_headers = Arc::new(Mutex::new(HashMap::new()));
        let handler: Arc<Handler> = Arc::new(handler);

        let server = Self {
            addr,
            hit_count: Arc::clone(&hit_count),
            concurrent_max: Arc::clone(&concurrent_max),
            seen_headers: Arc::clone(&seen_headers),
        };

        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let handler = Arc::clone(&handler);
                let hit_count = Arc::clone(&hit_count);
                let active_count = Arc::clone(&active_count);
                let concurrent_max = Arc::clone(&concurrent_max);
                let seen_headers = Arc::clone(&seen_headers);
                thread::spawn(move || {
                    hit_count.fetch_add(1, Ordering::SeqCst);
                    let request = read_request(&stream);
                    if let Some(request) = request {
                        {
                            let mut headers = seen_headers
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            for (name, value) in &request.headers {
                                headers.entry(name.clone()).or_insert_with(|| value.clone());
                            }
                        }
                        // Concurrency is measured around the handler only:
                        // the response is written *after* the decrement, so
                        // the client cannot observe a response while a
                        // previous request still counts as active.
                        let active = active_count.fetch_add(1, Ordering::SeqCst) + 1;
                        concurrent_max.fetch_max(active, Ordering::SeqCst);
                        let response = handler(&request);
                        active_count.fetch_sub(1, Ordering::SeqCst);
                        let _ = response.write_to(&mut stream);
                    }
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                });
            }
        });

        server
    }

    /// `http://ip:port` base url of the server.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Total number of requests served so far.
    pub fn hit_count(&self) -> usize {
        self.hit_count.load(Ordering::SeqCst)
    }

    /// Highest number of handlers running at the same time.
    pub fn concurrent_max(&self) -> usize {
        self.concurrent_max.load(Ordering::SeqCst)
    }

    /// The first seen value of a request header.
    pub fn seen_header(&self, name: &str) -> Option<String> {
        self.seen_headers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&name.to_ascii_lowercase())
            .cloned()
    }
}

fn read_request(mut stream: &TcpStream) -> Option<TestRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8_lossy(&buffer);
    let mut lines = text.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let _method = parts.next()?;
    let path = parts.next()?.to_owned();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    Some(TestRequest { path, headers })
}

/// HTML page used across tests: an icon image plus two titled links.
pub fn sample_page(icon_src: &str) -> String {
    format!(
        r#"<html><head><title>t</title></head><body>
        <img class="icon" src="{icon_src}">
        <a class="title" href="/a">A</a>
        <a class="title" href="/b">B</a>
        </body></html>"#
    )
}
