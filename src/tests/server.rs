//! Minimal hand-rolled HTTP test server used by the tests.
//!
//! It avoids any extra dependency: a `std::net::TcpListener` serves one
//! thread per connection and delegates every request to the handler
//! closure.

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
    body: String,
    content_length_override: Option<usize>,
}

impl TestResponse {
    /// HTTP 200 response with the given body.
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            content_length_override: None,
        }
    }

    /// Response with a custom status code and body.
    pub fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            content_length_override: None,
        }
    }

    /// Lies about the `Content-Length` (larger than the real body) so the
    /// client body read fails mid-flight.
    pub fn truncated_body(body: impl Into<String>) -> Self {
        let body = body.into();
        Self {
            status: 200,
            content_length_override: Some(body.len() + 50),
            body,
        }
    }

    fn write_to(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        let head = if let Some(length) = self.content_length_override {
            format!(
                "HTTP/1.1 {} X\r\nContent-Type: text/html\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n",
                self.status
            )
        } else {
            format!(
                "HTTP/1.1 {} X\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                self.status,
                self.body.len()
            )
        };
        stream.write_all(head.as_bytes())?;
        stream.write_all(self.body.as_bytes())?;
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
                    let active = active_count.fetch_add(1, Ordering::SeqCst) + 1;
                    concurrent_max.fetch_max(active, Ordering::SeqCst);
                    let request = read_request(&stream);
                    if let Some(request) = request {
                        for (name, value) in &request.headers {
                            seen_headers
                                .lock()
                                .expect("headers lock")
                                .entry(name.clone())
                                .or_insert_with(|| value.clone());
                        }
                        let response = handler(&request);
                        let _ = response.write_to(&mut stream);
                    }
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    active_count.fetch_sub(1, Ordering::SeqCst);
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

    /// Highest number of requests handled at the same time.
    pub fn concurrent_max(&self) -> usize {
        self.concurrent_max.load(Ordering::SeqCst)
    }

    /// The first seen value of a request header.
    pub fn seen_header(&self, name: &str) -> Option<String> {
        self.seen_headers
            .lock()
            .expect("headers lock")
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
