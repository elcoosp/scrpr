//! Integration-style tests exercising every public path against a local
//! hand-rolled HTTP server (no external test dependencies).

mod server;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::{
    ElementExt, RemoteScrapable, RemoteScrapeError, RemoteScraper, remote_scrape, scrape_html,
    scrape_html_all,
};
use server::{TestResponse, TestServer, sample_page};

/// Scrapes the `src` attribute of `img.icon` into a parsed [`url::Url`].
///
/// `base` is prepended to ids to build absolute urls pointing at the local
/// test server; offline tests use [`Icon::offline`].
struct Icon {
    base: String,
}

impl Icon {
    fn with_base(base: &str) -> Self {
        Self {
            base: base.to_owned(),
        }
    }

    fn offline() -> Self {
        Self {
            base: String::new(),
        }
    }
}

impl RemoteScrapable for Icon {
    type Output = url::Url;

    fn id_url(&self, id: &str) -> String {
        format!("{}/app/{id}", self.base)
    }

    fn elem_into(&self, elem: scraper::ElementRef<'_>) -> Result<Self::Output, RemoteScrapeError> {
        let src = elem.required_attr("src")?;
        Ok(url::Url::parse(src)?)
    }

    fn res_selector(&self) -> &'static str {
        "img.icon"
    }
}

/// [`Links`]-typed sibling of [`ToggleSelector`]: exercises every
/// `String`-output internal path with both a valid and an invalid selector.
struct StrToggle {
    valid: bool,
    base: String,
}

impl StrToggle {
    fn invalid() -> Self {
        Self {
            valid: false,
            base: String::new(),
        }
    }
}

impl RemoteScrapable for StrToggle {
    type Output = String;

    fn id_url(&self, id: &str) -> String {
        format!("{}/str-toggle/{id}", self.base)
    }

    fn elem_into(&self, elem: scraper::ElementRef<'_>) -> Result<Self::Output, RemoteScrapeError> {
        elem.required_attr("href").map(ToOwned::to_owned)
    }

    fn res_selector(&self) -> &'static str {
        if self.valid {
            "a.title"
        } else {
            "<<<not a selector either>>>"
        }
    }
}

/// Scrapes the `href` attribute of every `a.title` link.
struct Links {
    base: String,
}

impl Links {
    fn with_base(base: &str) -> Self {
        Self {
            base: base.to_owned(),
        }
    }

    fn offline() -> Self {
        Self {
            base: String::new(),
        }
    }
}

impl RemoteScrapable for Links {
    type Output = String;

    fn id_url(&self, id: &str) -> String {
        format!("{}/links/{id}", self.base)
    }

    fn elem_into(&self, elem: scraper::ElementRef<'_>) -> Result<Self::Output, RemoteScrapeError> {
        elem.required_attr("href").map(ToOwned::to_owned)
    }

    fn res_selector(&self) -> &'static str {
        "a.title"
    }
}

/// Scrapable whose selector can be switched between a valid and an invalid
/// one, so a single generic instantiation exercises both code paths.
struct ToggleSelector {
    valid: bool,
    base: String,
}

impl ToggleSelector {
    fn invalid() -> Self {
        Self {
            valid: false,
            base: String::new(),
        }
    }
}

impl RemoteScrapable for ToggleSelector {
    type Output = url::Url;

    fn id_url(&self, id: &str) -> String {
        format!("{}/toggle/{id}", self.base)
    }

    fn elem_into(&self, elem: scraper::ElementRef<'_>) -> Result<Self::Output, RemoteScrapeError> {
        Ok(url::Url::parse(elem.required_attr("src")?)?)
    }

    fn res_selector(&self) -> &'static str {
        if self.valid {
            "img.icon"
        } else {
            "<<<not a selector>>>"
        }
    }
}

fn server_serving_icon(icon_src: String) -> TestServer {
    TestServer::spawn(move |request| {
        let id = request.path.rsplit('/').next().unwrap_or("");
        TestResponse::ok(sample_page(&format!("{icon_src}/{id}.png")))
    })
}

#[tokio::test]
async fn scrape_returns_results_in_input_order() {
    let server = server_serving_icon("https://img.example.net".to_owned());
    let scraper = RemoteScraper::builder().max_retries(0).build().unwrap();
    let icon = Icon::with_base(&server.url());
    let ids = vec!["x1".to_owned(), "x2".to_owned(), "x3".to_owned()];
    let results = scraper.scrape(&icon, &ids).await;
    let expected = ["x1", "x2", "x3"]
        .into_iter()
        .map(|id| Ok(url::Url::parse(&format!("https://img.example.net/{id}.png")).unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(results, expected);
}

#[tokio::test]
async fn scrape_accepts_str_and_string_ids() {
    let server = server_serving_icon("https://img.example.net".to_owned());
    let scraper = RemoteScraper::new();
    let icon = Icon::with_base(&server.url());
    let owned = ["a".to_owned(), "b".to_owned()];
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    let results = scraper.scrape(&icon, refs).await;
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(Result::is_ok));
}

#[tokio::test]
async fn scrape_empty_ids_gives_empty_results() {
    let scraper = RemoteScraper::new();
    let results = scraper.scrape(&Icon::offline(), Vec::<&str>::new()).await;
    assert_eq!(results, Vec::new());
}

#[tokio::test]
async fn scrape_maps_404_to_page_not_found() {
    let server = TestServer::spawn(|_| TestResponse::status(404, "gone"));
    let scraper = RemoteScraper::builder().max_retries(0).build().unwrap();
    let base = server.url();
    let icon = Icon::with_base(&base);
    let results = scraper.scrape(&icon, ["id1"]).await;
    assert_eq!(
        results,
        vec![Err(RemoteScrapeError::PageNotFound(format!(
            "{base}/app/id1"
        )))]
    );
}

#[tokio::test]
async fn scrape_maps_other_client_errors_to_http_status() {
    let server = TestServer::spawn(|_| TestResponse::status(403, "forbidden"));
    let scraper = RemoteScraper::builder().max_retries(0).build().unwrap();
    let icon = Icon::with_base(&server.url());
    let results = scraper.scrape(&icon, ["id1"]).await;
    assert_eq!(results, vec![Err(RemoteScrapeError::HttpStatus(403))]);
}

#[tokio::test]
async fn scrape_retries_server_errors_until_success() {
    let failures_left = Arc::new(AtomicUsize::new(2));
    let failures = Arc::clone(&failures_left);
    let server = TestServer::spawn(move |_| {
        if failures.fetch_sub(1, Ordering::SeqCst) > 0 {
            TestResponse::status(500, "boom")
        } else {
            TestResponse::ok(sample_page("https://img.example.net/ok.png"))
        }
    });
    let scraper = RemoteScraper::builder()
        .max_retries(3)
        .retry_delay(Duration::from_millis(10))
        .build()
        .unwrap();
    let icon = Icon::with_base(&server.url());
    let results = scraper.scrape(&icon, ["id1"]).await;
    assert_eq!(
        results,
        vec![Ok(
            url::Url::parse("https://img.example.net/ok.png").unwrap()
        )]
    );
    assert_eq!(server.hit_count(), 3);
}

#[tokio::test]
async fn scrape_retries_exhausted_surfaces_http_status() {
    let server = TestServer::spawn(|_| TestResponse::status(503, "down"));
    let scraper = RemoteScraper::builder()
        .max_retries(1)
        .retry_delay(Duration::from_millis(10))
        .build()
        .unwrap();
    let icon = Icon::with_base(&server.url());
    let results = scraper.scrape(&icon, ["id1"]).await;
    assert_eq!(results, vec![Err(RemoteScrapeError::HttpStatus(503))]);
    assert_eq!(server.hit_count(), 2);
}

#[tokio::test]
async fn scrape_retries_network_errors_then_fails() {
    // Bind then drop the listener: connections are refused from now on.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let scraper = RemoteScraper::builder()
        .max_retries(1)
        .retry_delay(Duration::from_millis(10))
        .build()
        .unwrap();
    let icon = Icon::with_base(&format!("http://{addr}"));
    let results = scraper.scrape(&icon, ["x"]).await;
    assert!(matches!(results[0], Err(RemoteScrapeError::Request(_))));
}

#[tokio::test]
async fn scrape_network_error_without_retries_fails_fast() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let scraper = RemoteScraper::builder().max_retries(0).build().unwrap();
    let icon = Icon::with_base(&format!("http://{addr}"));
    let results = scraper.scrape(&icon, ["x"]).await;
    assert!(matches!(results[0], Err(RemoteScrapeError::Request(_))));
}

#[tokio::test]
async fn scrape_maps_body_read_failure_to_request_error() {
    let server =
        TestServer::spawn(|_| TestResponse::truncated_body("<html><body>short</body></html>"));
    let scraper = RemoteScraper::builder().max_retries(0).build().unwrap();
    let icon = Icon::with_base(&server.url());
    let results = scraper.scrape(&icon, ["id1"]).await;
    assert!(matches!(results[0], Err(RemoteScrapeError::Request(_))));
}

#[tokio::test]
async fn scrape_timeout_surfaces_request_error() {
    let server = TestServer::spawn(|_| {
        std::thread::sleep(Duration::from_millis(300));
        TestResponse::ok(sample_page("https://img.example.net/slow.png"))
    });
    let scraper = RemoteScraper::builder()
        .timeout(Duration::from_millis(50))
        .max_retries(0)
        .build()
        .unwrap();
    let icon = Icon::with_base(&server.url());
    let results = scraper.scrape(&icon, ["id1"]).await;
    assert!(matches!(results[0], Err(RemoteScrapeError::Request(_))));
}

#[tokio::test]
async fn scrape_invalid_selector_yields_one_error_per_id() {
    let scraper = RemoteScraper::new();
    let results = scraper.scrape(&ToggleSelector::invalid(), ["a", "b"]).await;
    assert_eq!(results.len(), 2);
    for result in results {
        assert!(matches!(result, Err(RemoteScrapeError::SelectorParse(_))));
    }
}

#[tokio::test]
async fn scrape_reports_no_element_match() {
    let server = TestServer::spawn(|_| TestResponse::ok("<html><body></body></html>"));
    let scraper = RemoteScraper::builder().max_retries(0).build().unwrap();
    let icon = Icon::with_base(&server.url());
    let results = scraper.scrape(&icon, ["id1"]).await;
    assert_eq!(results, vec![Err(RemoteScrapeError::NoElementMatch)]);
}

#[tokio::test]
async fn scrape_never_exceeds_configured_concurrency() {
    let server = TestServer::spawn(|request| {
        std::thread::sleep(Duration::from_millis(50));
        let id = request.path.rsplit('/').next().unwrap_or("");
        TestResponse::ok(sample_page(&format!("https://img.example.net/{id}.png")))
    });
    let scraper = RemoteScraper::builder()
        .concurrency(2)
        .max_retries(0)
        .build()
        .unwrap();
    let icon = Icon::with_base(&server.url());
    let ids: Vec<String> = (0..10).map(|i| format!("id{i}")).collect();
    let results = scraper.scrape(&icon, &ids).await;
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
    let scraper = RemoteScraper::builder().concurrency(0).build().unwrap();
    let icon = Icon::with_base(&server.url());
    let ids: Vec<String> = (0..4).map(|i| format!("id{i}")).collect();
    let results = scraper.scrape(&icon, &ids).await;
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(server.concurrent_max(), 1);
}

#[tokio::test]
async fn builder_sends_configured_user_agent() {
    let server = TestServer::spawn(|_| TestResponse::ok(sample_page("https://x.test/i.png")));
    let scraper = RemoteScraper::builder()
        .user_agent("scrpr-test/1.0")
        .build()
        .unwrap();
    let icon = Icon::with_base(&server.url());
    let results = scraper.scrape(&icon, ["id1"]).await;
    assert!(results[0].is_ok());
    assert_eq!(
        server.seen_header("user-agent").as_deref(),
        Some("scrpr-test/1.0")
    );
}

#[test]
fn builder_rejects_invalid_user_agent() {
    let built = RemoteScraper::builder().user_agent("bad\nheader").build();
    assert!(matches!(built, Err(RemoteScrapeError::Request(_))));
}

#[test]
fn builder_setters_compose() {
    let scraper = RemoteScraper::builder()
        .timeout(Duration::from_secs(1))
        .concurrency(3)
        .max_retries(5)
        .retry_delay(Duration::from_millis(5))
        .build()
        .unwrap();
    let _ = format!("{scraper:?}");
}

#[tokio::test]
async fn remote_scrape_free_function_works() {
    let server = server_serving_icon("https://img.example.net".to_owned());
    let icon = Icon::with_base(&server.url());
    let results = remote_scrape(&icon, ["id1", "id2"]).await;
    let expected = ["id1", "id2"]
        .into_iter()
        .map(|id| Ok(url::Url::parse(&format!("https://img.example.net/{id}.png")).unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(results, expected);
}

#[tokio::test]
async fn scrape_valid_toggle_selector_remotely() {
    let server =
        TestServer::spawn(|_| TestResponse::ok(sample_page("https://img.example.net/toggled.png")));
    let scraper = RemoteScraper::builder().max_retries(0).build().unwrap();
    let toggle = ToggleSelector {
        valid: true,
        base: server.url(),
    };
    let results = scraper.scrape(&toggle, ["id1"]).await;
    assert_eq!(
        results,
        vec![Ok(
            url::Url::parse("https://img.example.net/toggled.png").unwrap()
        )]
    );
}

#[tokio::test]
async fn scrape_invalid_str_toggle_yields_one_error_per_id() {
    let scraper = RemoteScraper::new();
    let results = scraper.scrape(&StrToggle::invalid(), ["a", "b"]).await;
    assert_eq!(results.len(), 2);
    for result in results {
        assert!(matches!(result, Err(RemoteScrapeError::SelectorParse(_))));
    }
}

#[tokio::test]
async fn scrape_links_remotely_and_no_match_reports_error() {
    let server =
        TestServer::spawn(|_| TestResponse::ok(sample_page("https://img.example.net/ignored.png")));
    let scraper = RemoteScraper::builder().max_retries(0).build().unwrap();
    let links = Links::with_base(&server.url());
    assert_eq!(
        scraper.scrape(&links, ["x"]).await,
        vec![Ok("/a".to_owned())]
    );

    let empty_server = TestServer::spawn(|_| TestResponse::ok("<html><body></body></html>"));
    let empty_links = Links::with_base(&empty_server.url());
    assert_eq!(
        scraper.scrape(&empty_links, ["x"]).await,
        vec![Err(RemoteScrapeError::NoElementMatch)]
    );
}

#[test]
fn scrape_html_invalid_str_toggle_errors() {
    let err = scrape_html(&StrToggle::invalid(), "<html></html>").unwrap_err();
    assert!(matches!(err, RemoteScrapeError::SelectorParse(_)));
}

#[test]
fn scrape_html_reads_first_match() {
    let html = sample_page("https://img.example.net/one.png");
    let icon = scrape_html(&Icon::offline(), &html).unwrap();
    assert_eq!(icon.as_str(), "https://img.example.net/one.png");
}

#[test]
fn scrape_html_errors_when_selector_invalid() {
    let err = scrape_html(&ToggleSelector::invalid(), "<html></html>").unwrap_err();
    assert!(matches!(err, RemoteScrapeError::SelectorParse(_)));
}

#[test]
fn scrape_html_errors_when_no_element_matches() {
    let err = scrape_html(&Links::offline(), "<html><body></body></html>").unwrap_err();
    assert_eq!(err, RemoteScrapeError::NoElementMatch);
}

#[test]
fn scrape_html_links_first_match() {
    let html = sample_page("https://img.example.net/one.png");
    let link = scrape_html(&Links::offline(), &html).unwrap();
    assert_eq!(link, "/a");
}

#[test]
fn scrape_html_propagates_url_parse_errors() {
    let html = sample_page("::::not-a-url");
    let err = scrape_html(&Icon::offline(), &html).unwrap_err();
    assert!(matches!(err, RemoteScrapeError::UrlParse(_)));
}

#[test]
fn scrape_html_propagates_missing_attribute_errors() {
    let html = "<html><body><img class=\"icon\"></body></html>";
    let err = scrape_html(&Icon::offline(), html).unwrap_err();
    assert_eq!(err, RemoteScrapeError::MissingAttribute("src".to_owned()));
}

#[test]
fn scrape_html_all_collects_every_match() {
    let html = sample_page("https://img.example.net/one.png");
    let links = scrape_html_all(&Links::offline(), &html).unwrap();
    assert_eq!(links, vec!["/a".to_owned(), "/b".to_owned()]);
    let icons = scrape_html_all(&Icon::offline(), &html).unwrap();
    assert_eq!(
        icons,
        vec![url::Url::parse("https://img.example.net/one.png").unwrap()]
    );
}

#[test]
fn scrape_html_all_errors_when_no_element_matches() {
    let empty = "<html><body></body></html>";
    let err = scrape_html_all(&Links::offline(), empty).unwrap_err();
    assert_eq!(err, RemoteScrapeError::NoElementMatch);
    let err = scrape_html_all(&Icon::offline(), empty).unwrap_err();
    assert_eq!(err, RemoteScrapeError::NoElementMatch);
}

#[test]
fn scrape_html_all_errors_when_selector_invalid() {
    let empty = "<html><body></body></html>";
    let err = scrape_html_all(&ToggleSelector::invalid(), empty).unwrap_err();
    assert!(matches!(err, RemoteScrapeError::SelectorParse(_)));
    let err = scrape_html_all(&StrToggle::invalid(), empty).unwrap_err();
    assert!(matches!(err, RemoteScrapeError::SelectorParse(_)));
}

#[test]
fn scrape_html_all_propagates_element_errors() {
    let html = "<html><body>\
                <img class=\"icon\" src=\"https://ok.test/a.png\">\
                <img class=\"icon\" src=\"::::bad\">\
                </body></html>";
    let err = scrape_html_all(&Icon::offline(), html).unwrap_err();
    assert!(matches!(err, RemoteScrapeError::UrlParse(_)));
}

#[test]
fn scrape_html_all_propagates_missing_attribute_errors() {
    let html = "<html><body><a class=\"title\">no href</a></body></html>";
    let err = scrape_html_all(&Links::offline(), html).unwrap_err();
    assert_eq!(err, RemoteScrapeError::MissingAttribute("href".to_owned()));
}

#[test]
fn default_constants_are_sane() {
    assert_eq!(crate::DEFAULT_CONCURRENCY, 8);
    assert_eq!(crate::DEFAULT_MAX_RETRIES, 2);
    assert_eq!(crate::DEFAULT_TIMEOUT, Duration::from_secs(30));
    assert_eq!(crate::DEFAULT_RETRY_DELAY, Duration::from_millis(200));
}
