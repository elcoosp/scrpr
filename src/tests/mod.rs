//! Integration-style tests exercising every public path against a local
//! hand-rolled HTTP server (no external test dependencies).

mod server;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;

use crate::{
    ElementExt, Extract, Page, ScrapeError, Scraper, extract, fetch, fetch_as, field, scrape_html,
    scrape_html_with,
};
use server::{TestResponse, TestServer, sample_page};

/// Extracts the `src` attribute of `img.icon` into a parsed [`url::Url`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Icon(url::Url);

impl Extract for Icon {
    fn extract(page: &Page) -> Result<Self, ScrapeError> {
        let elem = page.select_required("img.icon")?;
        Ok(Self(url::Url::parse(elem.required_attr("src")?)?))
    }
}

/// Extracts the `href` of every `a.title` link.
struct Links;

impl Extract for Links {
    fn extract(page: &Page) -> Result<Self, ScrapeError> {
        let _ = page.select_required("a.title")?;
        Ok(Self)
    }
}

/// Extracts structured data from the first ld+json payload.
struct Structured {
    name: String,
}

#[cfg(feature = "json")]
impl Extract for Structured {
    fn extract(page: &Page) -> Result<Self, ScrapeError> {
        let values = field::json_ld().get(page)?;
        let first = values
            .first()
            .ok_or_else(|| ScrapeError::Json("no ld+json payload".to_owned()))?;
        Ok(Self {
            name: first["name"]
                .as_str()
                .ok_or_else(|| ScrapeError::Json("missing name".to_owned()))?
                .to_owned(),
        })
    }
}

fn icon_server() -> (TestServer, String) {
    let server = TestServer::spawn(move |request| {
        let id = request.path.rsplit('/').next().unwrap_or("");
        TestResponse::ok(sample_page(&format!("https://img.example.net/{id}.png")))
    });
    let base = server.url();
    (server, base)
}

// ---------------------------------------------------------------------------
// Offline extraction (sanity checks; unit tests cover the details)
// ---------------------------------------------------------------------------

#[test]
fn offline_page_and_extractors() {
    let page = Page::parse_with_base(
        &sample_page("https://img.example.net/one.png"),
        "https://s.test",
    )
    .unwrap();
    assert_eq!(
        field::attr_url("img.icon", "src").get(&page).unwrap(),
        url::Url::parse("https://img.example.net/one.png").unwrap()
    );
    assert_eq!(
        field::text_all("a.title").get(&page).unwrap(),
        vec!["A".to_owned(), "B".to_owned()]
    );
    let (title, links) = extract(&page, (field::title(), field::links())).unwrap();
    assert_eq!(title, "t");
    assert_eq!(links.len(), 2);
}

#[test]
fn offline_extract_structs() {
    let html = sample_page("https://img.example.net/one.png");
    let icon = scrape_html::<Icon>(&html).unwrap();
    assert_eq!(
        icon,
        Icon(url::Url::parse("https://img.example.net/one.png").unwrap())
    );
    let _ = scrape_html::<Links>(&html).unwrap();
    let link = scrape_html_with(&html, &field::attr("a.title", "href")).unwrap();
    assert_eq!(link, "/a");
    assert!(scrape_html::<Icon>("<html><body></body></html>").is_err());
}

#[cfg(feature = "json")]
#[test]
fn offline_structured_data() {
    let html = r#"<html><head>
        <script type="application/ld+json">{"@type":"Product","name":"Chair"}</script>
        </head><body></body></html>"#;
    let structured = scrape_html::<Structured>(html).unwrap();
    assert_eq!(structured.name, "Chair");
}

// ---------------------------------------------------------------------------
// Single fetch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fetch_returns_page_with_source_url() {
    let (server, base) = icon_server();
    let scraper = Scraper::builder().max_retries(0).build().unwrap();
    let page = scraper.fetch(&format!("{base}/app/id1")).await.unwrap();
    assert_eq!(page.url().unwrap().as_str(), format!("{base}/app/id1"));
    let icon = Icon::extract(&page).unwrap();
    assert_eq!(
        icon,
        Icon(url::Url::parse("https://img.example.net/id1.png").unwrap())
    );
    assert_eq!(server.hit_count(), 1);
}

#[tokio::test]
async fn fetch_rejects_invalid_urls_without_requests() {
    let (server, _base) = icon_server();
    let scraper = Scraper::new().unwrap();
    let err = scraper.fetch("::nope::").await.unwrap_err();
    assert!(matches!(err, ScrapeError::UrlParse { .. }));
    assert_eq!(server.hit_count(), 0);
}

#[tokio::test]
async fn fetch_maps_404_to_page_not_found() {
    let server = TestServer::spawn(|_| TestResponse::status(404, "gone"));
    let scraper = Scraper::builder().max_retries(0).build().unwrap();
    let base = format!("{}/", server.url());
    let err = scraper.fetch(&base).await.unwrap_err();
    assert_eq!(err, ScrapeError::PageNotFound { url: base.clone() });
    assert_eq!(server.hit_count(), 1, "404 is never retried");
}

#[tokio::test]
async fn fetch_maps_other_client_errors_to_http_status() {
    let server = TestServer::spawn(|_| TestResponse::status(403, "forbidden"));
    let scraper = Scraper::builder().max_retries(0).build().unwrap();
    let base = format!("{}/", server.url());
    assert_eq!(
        scraper.fetch(&base).await.unwrap_err(),
        ScrapeError::Http {
            status: 403,
            url: base
        }
    );
}

#[tokio::test]
async fn fetch_retries_server_errors_until_success() {
    let failures_left = Arc::new(AtomicUsize::new(2));
    let failures = Arc::clone(&failures_left);
    let server = TestServer::spawn(move |_| {
        if failures.fetch_sub(1, Ordering::SeqCst) > 0 {
            TestResponse::status(500, "boom")
        } else {
            TestResponse::ok(sample_page("https://img.example.net/ok.png"))
        }
    });
    let scraper = Scraper::builder()
        .max_retries(3)
        .retry_delay(Duration::from_millis(10))
        .build()
        .unwrap();
    let base = server.url();
    let icon = scraper.fetch_as::<Icon>(&base).await.unwrap();
    assert_eq!(
        icon,
        Icon(url::Url::parse("https://img.example.net/ok.png").unwrap())
    );
    assert_eq!(server.hit_count(), 3);
}

#[tokio::test]
async fn fetch_retries_exhausted_surfaces_http_status() {
    let server = TestServer::spawn(|_| TestResponse::status(503, "down"));
    let scraper = Scraper::builder()
        .max_retries(1)
        .retry_delay(Duration::from_millis(10))
        .build()
        .unwrap();
    let base = format!("{}/", server.url());
    assert_eq!(
        scraper.fetch(&base).await.unwrap_err(),
        ScrapeError::Http {
            status: 503,
            url: base
        }
    );
    assert_eq!(server.hit_count(), 2);
}

#[tokio::test]
async fn fetch_retries_408_like_other_retryable_statuses() {
    let failures_left = Arc::new(AtomicUsize::new(1));
    let failures = Arc::clone(&failures_left);
    let server = TestServer::spawn(move |_| {
        if failures.fetch_sub(1, Ordering::SeqCst) > 0 {
            TestResponse::status(408, "timeout")
        } else {
            TestResponse::ok("<html><body></body></html>")
        }
    });
    let scraper = Scraper::builder()
        .max_retries(1)
        .retry_delay(Duration::from_millis(5))
        .build()
        .unwrap();
    assert!(scraper.fetch(&server.url()).await.is_ok());
    assert_eq!(server.hit_count(), 2);
}

#[tokio::test]
async fn fetch_retries_network_errors_then_fails() {
    // Bind then drop the listener: connections are refused from now on.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let scraper = Scraper::builder()
        .max_retries(1)
        .retry_delay(Duration::from_millis(10))
        .build()
        .unwrap();
    let err = scraper.fetch(&format!("http://{addr}")).await.unwrap_err();
    assert!(matches!(err, ScrapeError::Network { .. }));
}

#[tokio::test]
async fn fetch_network_error_without_retries_fails_fast() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let scraper = Scraper::builder().max_retries(0).build().unwrap();
    assert!(matches!(
        scraper.fetch(&format!("http://{addr}")).await.unwrap_err(),
        ScrapeError::Network { .. }
    ));
}

#[tokio::test]
async fn fetch_maps_body_read_failure_to_network_error() {
    let server =
        TestServer::spawn(|_| TestResponse::truncated_body("<html><body>short</body></html>"));
    let scraper = Scraper::builder().max_retries(0).build().unwrap();
    assert!(matches!(
        scraper.fetch(&server.url()).await.unwrap_err(),
        ScrapeError::Network { .. }
    ));
}

#[tokio::test]
async fn fetch_timeout_surfaces_timeout_error() {
    let server = TestServer::spawn(|_| {
        std::thread::sleep(Duration::from_millis(300));
        TestResponse::ok(sample_page("https://img.example.net/slow.png"))
    });
    let scraper = Scraper::builder()
        .timeout(Duration::from_millis(50))
        .max_retries(0)
        .build()
        .unwrap();
    let err = scraper.fetch(&server.url()).await.unwrap_err();
    assert!(matches!(err, ScrapeError::Timeout { .. }));
}

#[tokio::test]
async fn fetch_caps_bodies_declared_too_large() {
    let server = TestServer::spawn(|_| {
        TestResponse::ok(format!("<html><body>{}</body></html>", "x".repeat(100)))
    });
    let scraper = Scraper::builder().max_body_size(10).build().unwrap();
    let base = format!("{}/", server.url());
    let err = scraper.fetch(&base).await.unwrap_err();
    assert_eq!(
        err,
        ScrapeError::BodyTooLarge {
            url: base,
            limit: 10
        }
    );
}

#[tokio::test]
async fn fetch_caps_oversized_chunked_bodies_mid_stream() {
    let server = TestServer::spawn(|_| {
        TestResponse::chunked(format!("<html><body>{}</body></html>", "y".repeat(100)))
    });
    let scraper = Scraper::builder().max_body_size(10).build().unwrap();
    assert!(matches!(
        scraper.fetch(&server.url()).await.unwrap_err(),
        ScrapeError::BodyTooLarge { .. }
    ));
}

#[tokio::test]
async fn fetch_decodes_non_utf8_charsets() {
    // "Café" in ISO-8859-1: the é is byte 0xE9.
    let mut body = b"<html><body><p>Caf".to_vec();
    body.push(0xE9);
    body.extend_from_slice(b"</p></body></html>");
    let server = TestServer::spawn(move |_| {
        TestResponse::bytes(200, "text/html; charset=iso-8859-1", body.clone())
    });
    let scraper = Scraper::new().unwrap();
    let page = scraper.fetch(&server.url()).await.unwrap();
    assert_eq!(field::text("p").get(&page).unwrap(), "Café");
}

// ---------------------------------------------------------------------------
// Politeness: Retry-After and rate limiting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fetch_respects_retry_after_on_429() {
    let failures_left = Arc::new(AtomicUsize::new(1));
    let failures = Arc::clone(&failures_left);
    let server = TestServer::spawn(move |_| {
        if failures.fetch_sub(1, Ordering::SeqCst) > 0 {
            TestResponse::retry_after(1)
        } else {
            TestResponse::ok(sample_page("https://img.example.net/ok.png"))
        }
    });
    let scraper = Scraper::builder()
        .max_retries(2)
        .retry_delay(Duration::from_millis(1))
        .build()
        .unwrap();
    let start = Instant::now();
    let result = scraper.fetch(&server.url()).await;
    let elapsed = start.elapsed();
    assert!(result.is_ok());
    assert_eq!(server.hit_count(), 2);
    assert!(
        elapsed >= Duration::from_millis(950),
        "the server-demanded 1s wait must be honored, took {elapsed:?}"
    );
}

#[tokio::test]
async fn fetch_can_ignore_retry_after() {
    let failures_left = Arc::new(AtomicUsize::new(1));
    let failures = Arc::clone(&failures_left);
    let server = TestServer::spawn(move |_| {
        if failures.fetch_sub(1, Ordering::SeqCst) > 0 {
            TestResponse::retry_after(60)
        } else {
            TestResponse::ok(sample_page("https://img.example.net/ok.png"))
        }
    });
    let scraper = Scraper::builder()
        .max_retries(2)
        .retry_delay(Duration::from_millis(1))
        .max_retry_delay(Duration::from_millis(50))
        .respect_retry_after(false)
        .build()
        .unwrap();
    let start = Instant::now();
    let result = scraper.fetch(&server.url()).await;
    let elapsed = start.elapsed();
    assert!(result.is_ok());
    assert!(
        elapsed < Duration::from_secs(5),
        "must not wait 60s, took {elapsed:?}"
    );
}

#[tokio::test]
async fn rate_limiter_paces_concurrent_fetches() {
    let server = icon_server().0;
    let scraper = Scraper::builder()
        .min_request_interval(Duration::from_millis(60))
        .build()
        .unwrap();
    let base = server.url();
    let start = Instant::now();
    let (a, b, c) = tokio::join!(
        scraper.fetch(&base),
        scraper.fetch(&base),
        scraper.fetch(&base)
    );
    assert!(a.is_ok() && b.is_ok() && c.is_ok());
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(110),
        "three fetches at 60ms spacing need >= 120ms, took {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(2));
}

// ---------------------------------------------------------------------------
// Page cache
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cache_deduplicates_identical_urls() {
    let (server, base) = icon_server();
    let scraper = Scraper::builder().cache_capacity(8).build().unwrap();
    scraper.fetch(&base).await.unwrap();
    scraper.fetch(&base).await.unwrap();
    assert_eq!(server.hit_count(), 1);

    // A clone shares the same cache.
    scraper.clone().fetch(&base).await.unwrap();
    assert_eq!(server.hit_count(), 1);

    scraper.fetch(&format!("{base}/other")).await.unwrap();
    assert_eq!(server.hit_count(), 2);
}

#[tokio::test]
async fn cache_entries_expire_after_ttl() {
    let (server, base) = icon_server();
    let scraper = Scraper::builder()
        .cache_capacity(8)
        .cache_ttl(Duration::from_millis(40))
        .build()
        .unwrap();
    scraper.fetch(&base).await.unwrap();
    std::thread::sleep(Duration::from_millis(80));
    scraper.fetch(&base).await.unwrap();
    assert_eq!(server.hit_count(), 2);
}

#[tokio::test]
async fn failures_are_never_cached() {
    let server = TestServer::spawn(|_| TestResponse::status(404, "gone"));
    let scraper = Scraper::builder().cache_capacity(8).build().unwrap();
    assert!(scraper.fetch(&server.url()).await.is_err());
    assert!(scraper.fetch(&server.url()).await.is_err());
    assert_eq!(server.hit_count(), 2);
}

#[tokio::test]
async fn cache_is_disabled_by_default() {
    let (server, base) = icon_server();
    let scraper = Scraper::new().unwrap();
    scraper.fetch(&base).await.unwrap();
    scraper.fetch(&base).await.unwrap();
    assert_eq!(server.hit_count(), 2);
}

// ---------------------------------------------------------------------------
// Redirects, headers, cookies
// ---------------------------------------------------------------------------

#[tokio::test]
async fn redirects_are_followed_by_default() {
    let server = TestServer::spawn(|request| {
        if request.path == "/redirect" {
            TestResponse::redirect("/target")
        } else {
            TestResponse::ok(sample_page("https://img.example.net/redirected.png"))
        }
    });
    let scraper = Scraper::new().unwrap();
    let base = server.url();
    let icon = scraper
        .fetch_as::<Icon>(&format!("{base}/redirect"))
        .await
        .unwrap();
    assert_eq!(
        icon,
        Icon(url::Url::parse("https://img.example.net/redirected.png").unwrap())
    );
}

#[tokio::test]
async fn redirects_can_be_disabled() {
    let server = TestServer::spawn(|request| {
        if request.path == "/redirect" {
            TestResponse::redirect("/target")
        } else {
            TestResponse::ok("<html><body></body></html>")
        }
    });
    let scraper = Scraper::builder().follow_redirects(false).build().unwrap();
    let base = format!("{}/redirect", server.url());
    assert_eq!(
        scraper.fetch(&base).await.unwrap_err(),
        ScrapeError::Http {
            status: 302,
            url: base
        }
    );
}

#[tokio::test]
async fn default_user_agent_identifies_the_scraper() {
    let (server, base) = icon_server();
    let scraper = Scraper::new().unwrap();
    scraper.fetch(&base).await.unwrap();
    assert!(
        server
            .seen_header("user-agent")
            .unwrap()
            .starts_with("scrpr/")
    );
}

#[tokio::test]
async fn custom_user_agent_and_headers_are_sent() {
    let (server, base) = icon_server();
    let scraper = Scraper::builder()
        .user_agent("scrpr-test/1.0")
        .header("x-scrpr-test", "42")
        .build()
        .unwrap();
    scraper.fetch(&base).await.unwrap();
    assert_eq!(
        server.seen_header("user-agent").as_deref(),
        Some("scrpr-test/1.0")
    );
    assert_eq!(server.seen_header("x-scrpr-test").as_deref(), Some("42"));
}

#[tokio::test]
async fn cookie_store_round_trips_cookies() {
    let server = TestServer::spawn(|_| {
        TestResponse::ok(sample_page("https://img.example.net/x.png"))
            .with_header("Set-Cookie", "session=abc; Path=/")
    });
    let scraper = Scraper::builder().cookie_store(true).build().unwrap();
    let base = server.url();
    scraper.fetch(&base).await.unwrap();
    scraper.fetch(&base).await.unwrap();
    assert_eq!(server.seen_header("cookie").as_deref(), Some("session=abc"));
}

// ---------------------------------------------------------------------------
// Batch APIs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scrape_returns_results_in_input_order() {
    let (server, base) = icon_server();
    let scraper = Scraper::builder().max_retries(0).build().unwrap();
    let urls = [
        format!("{base}/x1"),
        format!("{base}/x2"),
        format!("{base}/x3"),
    ];
    let results = scraper.scrape::<Icon, _, _>(&urls).await;
    let expected = ["x1", "x2", "x3"]
        .into_iter()
        .map(|id| {
            Ok(Icon(
                url::Url::parse(&format!("https://img.example.net/{id}.png")).unwrap(),
            ))
        })
        .collect::<Vec<_>>();
    assert_eq!(results, expected);
    assert_eq!(server.hit_count(), 3);
}

#[tokio::test]
async fn scrape_accepts_str_and_string_urls() {
    let (_server, base) = icon_server();
    let scraper = Scraper::new().unwrap();
    let owned = [format!("{base}/a"), format!("{base}/b")];
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    let results = scraper.scrape::<Icon, _, _>(refs).await;
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(Result::is_ok));
}

#[tokio::test]
async fn scrape_empty_input_gives_empty_results() {
    let scraper = Scraper::new().unwrap();
    let results = scraper.scrape::<Icon, _, _>(Vec::<&str>::new()).await;
    assert_eq!(results, Vec::new());
}

#[tokio::test]
async fn scrape_reports_invalid_urls_without_fetching() {
    let (server, _base) = icon_server();
    let scraper = Scraper::new().unwrap();
    let results = scraper.scrape::<Icon, _, _>(["::bad::"]).await;
    assert!(matches!(results[0], Err(ScrapeError::UrlParse { .. })));
    assert_eq!(server.hit_count(), 0);
}

#[tokio::test]
async fn scrape_mixed_success_and_failure() {
    let server = TestServer::spawn(|request| {
        if request.path.ends_with("/missing") {
            TestResponse::status(404, "gone")
        } else {
            TestResponse::ok(sample_page("https://img.example.net/x.png"))
        }
    });
    let scraper = Scraper::builder().max_retries(0).build().unwrap();
    let base = server.url();
    let results = scraper
        .scrape::<Icon, _, _>([format!("{base}/present"), format!("{base}/missing")])
        .await;
    assert!(results[0].is_ok());
    assert_eq!(
        results[1],
        Err(ScrapeError::PageNotFound {
            url: format!("{base}/missing")
        })
    );
}

#[tokio::test]
async fn scrape_never_exceeds_configured_concurrency() {
    let server = TestServer::spawn(|request| {
        std::thread::sleep(Duration::from_millis(50));
        let id = request.path.rsplit('/').next().unwrap_or("");
        TestResponse::ok(sample_page(&format!("https://img.example.net/{id}.png")))
    });
    let scraper = Scraper::builder()
        .concurrency(2)
        .max_retries(0)
        .build()
        .unwrap();
    let base = server.url();
    let urls: Vec<String> = (0..10).map(|i| format!("{base}/id{i}")).collect();
    let results = scraper.scrape::<Icon, _, _>(&urls).await;
    assert_eq!(results.len(), 10);
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(server.hit_count(), 10);
    assert_eq!(server.concurrent_max(), 2);
}

#[tokio::test]
async fn scrape_clamps_zero_concurrency_to_one() {
    let server = TestServer::spawn(|request| {
        std::thread::sleep(Duration::from_millis(20));
        let id = request.path.rsplit('/').next().unwrap_or("");
        TestResponse::ok(sample_page(&format!("https://img.example.net/{id}.png")))
    });
    let scraper = Scraper::builder().concurrency(0).build().unwrap();
    let base = server.url();
    let urls: Vec<String> = (0..4).map(|i| format!("{base}/id{i}")).collect();
    let results = scraper.scrape::<Icon, _, _>(&urls).await;
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(server.concurrent_max(), 1);
}

#[tokio::test]
async fn scrape_stream_yields_positions_in_order() {
    let (_server, base) = icon_server();
    let scraper = Scraper::builder().max_retries(0).build().unwrap();
    let urls: Vec<String> = (0..6).map(|i| format!("{base}/id{i}")).collect();
    let mut stream = scraper.scrape_stream::<Icon, _, _>(&urls);
    let mut collected = Vec::new();
    while let Some((position, result)) = stream.next().await {
        assert!(result.is_ok());
        collected.push(position);
    }
    assert_eq!(collected, vec![0, 1, 2, 3, 4, 5]);
}

#[tokio::test]
async fn scrape_stream_surfaces_url_errors_in_position() {
    let scraper = Scraper::new().unwrap();
    let mut stream = scraper.scrape_stream::<Icon, _, _>(["::bad::"]);
    let (position, result) = stream.next().await.unwrap();
    assert_eq!(position, 0);
    assert!(matches!(result, Err(ScrapeError::UrlParse { .. })));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn scrape_extracts_structured_data_remotely() {
    let server = TestServer::spawn(|_| {
        TestResponse::ok(
            r#"<html><head>
            <script type="application/ld+json">{"@type":"Product","name":"Desk"}</script>
            </head><body></body></html>"#,
        )
    });
    let scraper = Scraper::new().unwrap();
    let structured = scraper.fetch_as::<Structured>(&server.url()).await.unwrap();
    assert_eq!(structured.name, "Desk");
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn free_fetch_helpers_work() {
    let (_server, base) = icon_server();
    let page = fetch(&format!("{base}/app/id1")).await.unwrap();
    assert!(page.url().is_some());
    let icon = fetch_as::<Icon>(&format!("{base}/app/id1")).await.unwrap();
    assert_eq!(
        icon,
        Icon(url::Url::parse("https://img.example.net/id1.png").unwrap())
    );
}

#[tokio::test]
async fn scraper_debug_and_default_build() {
    let scraper = Scraper::new().unwrap();
    assert!(format!("{scraper:?}").contains("Scraper"));
    let _ = format!("{:?}", Scraper::builder());
}

#[test]
fn default_constants_are_sane() {
    assert_eq!(crate::DEFAULT_CONCURRENCY, 8);
    assert_eq!(crate::DEFAULT_MAX_RETRIES, 2);
    assert_eq!(crate::DEFAULT_TIMEOUT, Duration::from_secs(30));
    assert_eq!(crate::DEFAULT_RETRY_DELAY, Duration::from_millis(200));
    assert_eq!(crate::DEFAULT_MAX_RETRY_DELAY, Duration::from_secs(5));
    assert_eq!(crate::DEFAULT_MAX_BODY_SIZE, 10 * 1024 * 1024);
    assert_eq!(crate::DEFAULT_CACHE_TTL, Duration::from_secs(300));
}
