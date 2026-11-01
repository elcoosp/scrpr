//! Configurable scraper with bounded concurrency, retries and timeouts.

use std::time::Duration;

use futures::StreamExt;
use scraper::Selector;

use crate::error::RemoteScrapeError;
use crate::traits::RemoteScrapable;

/// Default maximum number of ids scraped concurrently.
pub const DEFAULT_CONCURRENCY: usize = 8;
/// Default per-request timeout applied by [`RemoteScraper::new`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default number of retries per request (so 3 total attempts).
pub const DEFAULT_MAX_RETRIES: u32 = 2;
/// Default base delay between retries; the delay doubles on every attempt.
pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Reusable, configurable scraper.
///
/// One instance shares a single [`reqwest::Client`] (and therefore its
/// connection pool) across every call, and can be cloned cheaply.
///
/// # Example
///
/// ```
/// use std::time::Duration;
///
/// use scrpr::{ElementExt, RemoteScraper, RemoteScrapable, RemoteScrapeError};
///
/// struct Title;
///
/// impl RemoteScrapable for Title {
///     type Output = String;
///
///     fn id_url(&self, id: &str) -> String {
///         format!("https://example.com/{id}")
///     }
///
///     fn elem_into(
///         &self,
///         elem: scraper::ElementRef<'_>,
///     ) -> Result<Self::Output, RemoteScrapeError> {
///         Ok(elem.text_content())
///     }
///
///     fn res_selector(&self) -> &str {
///         "h1"
///     }
/// }
///
/// # async fn demo() {
/// let scraper = RemoteScraper::builder()
///     .timeout(Duration::from_secs(10))
///     .max_retries(1)
///     .concurrency(4)
///     .build()
///     .expect("valid configuration");
///
/// // Results are always returned in the input ids order.
/// let results = scraper.scrape(&Title, ["a", "b"]).await;
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RemoteScraper {
    client: reqwest::Client,
    concurrency: usize,
    max_retries: u32,
    retry_delay: Duration,
}

impl Default for RemoteScraper {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            concurrency: DEFAULT_CONCURRENCY,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_delay: DEFAULT_RETRY_DELAY,
        }
    }
}

impl RemoteScraper {
    /// Creates a scraper with all default settings.
    ///
    /// See [`RemoteScraperBuilder`] to configure the timeout, user agent,
    /// concurrency or retries.
    ///
    /// # Panics
    ///
    /// Panics if the underlying TLS backend fails to initialize, mirroring
    /// [`reqwest::Client::new`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a builder allowing full configuration.
    #[must_use]
    pub fn builder() -> RemoteScraperBuilder {
        RemoteScraperBuilder::default()
    }

    /// Scrapes every id concurrently and returns the results in the input
    /// order (one entry per id).
    ///
    /// Concurrent requests are capped at the configured concurrency level.
    /// HTTP 404 answers yield [`RemoteScrapeError::PageNotFound`], other
    /// non-success statuses yield [`RemoteScrapeError::HttpStatus`], and
    /// server errors plus network failures are retried with an exponential
    /// backoff.
    ///
    /// An invalid [`RemoteScrapable::res_selector`] yields one
    /// [`RemoteScrapeError::SelectorParse`] entry per id instead of
    /// attempting any request.
    pub async fn scrape<T, I>(
        &self,
        scrapable: &T,
        ids: I,
    ) -> Vec<Result<T::Output, RemoteScrapeError>>
    where
        T: RemoteScrapable,
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let ids = ids
            .into_iter()
            .map(|id| id.as_ref().to_owned())
            .collect::<Vec<_>>();
        self.scrape_remote_dyn(scrapable, ids).await
    }

    /// Remote scraping shared by every public entry point: one
    /// instantiation per output type instead of one per scrapable type,
    /// which keeps the compiled code small and every branch testable.
    async fn scrape_remote_dyn<O>(
        &self,
        scrapable: &dyn RemoteScrapable<Output = O>,
        ids: Vec<String>,
    ) -> Vec<Result<O, RemoteScrapeError>> {
        let selector = match parse_selector(scrapable.res_selector()) {
            Ok(selector) => selector,
            Err(err) => return selector_errors(&err, ids),
        };
        futures::stream::iter(ids.into_iter().map(|id| {
            let id_url = scrapable.id_url(&id);
            let selector = selector.clone();
            async move {
                self.fetch_html(&id_url)
                    .await
                    .and_then(|html| scrape_first_erased(scrapable, &selector, &html))
            }
        }))
        .buffered(self.concurrency)
        .collect::<Vec<_>>()
        .await
    }
}

/// Scrapes one local HTML document, transforming the first element matching
/// the resource selector.
///
/// Useful to re-scrape already downloaded pages or to test implementations
/// offline.
///
/// # Errors
///
/// Errors with [`RemoteScrapeError::SelectorParse`] when the selector is
/// invalid and [`RemoteScrapeError::NoElementMatch`] when nothing matches.
#[must_use = "scraping without reading the result has no effect"]
pub fn scrape_html<T: RemoteScrapable>(
    scrapable: &T,
    html: &str,
) -> Result<T::Output, RemoteScrapeError> {
    scrape_html_dyn(scrapable, html)
}

/// Scrapes one local HTML document, transforming **every** element matching
/// the resource selector.
///
/// # Errors
///
/// Errors with [`RemoteScrapeError::SelectorParse`] when the selector is
/// invalid and [`RemoteScrapeError::NoElementMatch`] when nothing matches.
#[must_use = "scraping without reading the result has no effect"]
pub fn scrape_html_all<T: RemoteScrapable>(
    scrapable: &T,
    html: &str,
) -> Result<Vec<T::Output>, RemoteScrapeError> {
    scrape_html_all_dyn(scrapable, html)
}

/// Scrapes every id with a default, unconfigured [`RemoteScraper`].
///
/// Prefer building one [`RemoteScraper`] and reusing it when scraping
/// repeatedly: the default scraper rebuilds its connection pool on every
/// call.
pub async fn remote_scrape<T, I>(scrapable: &T, ids: I) -> Vec<Result<T::Output, RemoteScrapeError>>
where
    T: RemoteScrapable,
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    RemoteScraper::default().scrape(scrapable, ids).await
}

/// Configures and builds a [`RemoteScraper`].
///
/// # Example
///
/// ```
/// use std::time::Duration;
///
/// use scrpr::RemoteScraper;
///
/// let scraper = RemoteScraper::builder()
///     .user_agent("my-crawler/0.1")
///     .timeout(Duration::from_secs(5))
///     .build()
///     .expect("valid configuration");
/// ```
#[derive(Debug)]
pub struct RemoteScraperBuilder {
    client: reqwest::ClientBuilder,
    concurrency: usize,
    max_retries: u32,
    retry_delay: Duration,
}

impl Default for RemoteScraperBuilder {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder().timeout(DEFAULT_TIMEOUT),
            concurrency: DEFAULT_CONCURRENCY,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_delay: DEFAULT_RETRY_DELAY,
        }
    }
}

impl RemoteScraperBuilder {
    /// Sets the `User-Agent` header sent with every request.
    #[must_use]
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.client = self.client.user_agent(user_agent.into());
        self
    }

    /// Sets the timeout applied to each request (connect + read).
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.client = self.client.timeout(timeout);
        self
    }

    /// Sets the maximum number of ids fetched concurrently.
    ///
    /// Values below 1 are clamped to 1.
    #[must_use]
    pub const fn concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = if concurrency < 1 { 1 } else { concurrency };
        self
    }

    /// Sets how many times a failing request (network error or HTTP 5xx) is
    /// retried before surfacing the error.
    #[must_use]
    pub const fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Sets the base delay between retries; the delay doubles on every
    /// attempt (exponential backoff).
    #[must_use]
    pub const fn retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = retry_delay;
        self
    }

    /// Builds the configured [`RemoteScraper`].
    ///
    /// # Errors
    ///
    /// Errors with [`RemoteScrapeError::Request`] when the underlying
    /// [`reqwest::Client`] cannot be built (for example an invalid
    /// user agent header value).
    pub fn build(self) -> Result<RemoteScraper, RemoteScrapeError> {
        let client = self
            .client
            .build()
            .map_err(|err| RemoteScrapeError::Request(err.to_string()))?;
        Ok(RemoteScraper {
            client,
            concurrency: self.concurrency,
            max_retries: self.max_retries,
            retry_delay: self.retry_delay,
        })
    }
}

fn scrape_html_dyn<O>(
    scrapable: &dyn RemoteScrapable<Output = O>,
    html: &str,
) -> Result<O, RemoteScrapeError> {
    let selector = parse_selector(scrapable.res_selector())?;
    scrape_first_erased(scrapable, &selector, html)
}

fn scrape_html_all_dyn<O>(
    scrapable: &dyn RemoteScrapable<Output = O>,
    html: &str,
) -> Result<Vec<O>, RemoteScrapeError> {
    let selector = parse_selector(scrapable.res_selector())?;
    scrape_all_erased(scrapable, &selector, html)
}

fn parse_selector(selector_str: &str) -> Result<Selector, RemoteScrapeError> {
    Selector::parse(selector_str).map_err(|err| RemoteScrapeError::SelectorParse(err.to_string()))
}

fn selector_errors<O>(
    err: &RemoteScrapeError,
    ids: Vec<String>,
) -> Vec<Result<O, RemoteScrapeError>> {
    ids.into_iter().map(|_| Err(err.clone())).collect()
}

fn scrape_first_erased<O>(
    scrapable: &dyn RemoteScrapable<Output = O>,
    selector: &Selector,
    html: &str,
) -> Result<O, RemoteScrapeError> {
    let document = scraper::Html::parse_document(html);
    match document.select(selector).next() {
        Some(elem) => scrapable.elem_into(elem),
        None => Err(RemoteScrapeError::NoElementMatch),
    }
}

fn scrape_all_erased<O>(
    scrapable: &dyn RemoteScrapable<Output = O>,
    selector: &Selector,
    html: &str,
) -> Result<Vec<O>, RemoteScrapeError> {
    let document = scraper::Html::parse_document(html);
    let outputs = document
        .select(selector)
        .map(|elem| scrapable.elem_into(elem))
        .collect::<Result<Vec<_>, RemoteScrapeError>>()?;
    if outputs.is_empty() {
        return Err(RemoteScrapeError::NoElementMatch);
    }
    Ok(outputs)
}

impl RemoteScraper {
    async fn fetch_html(&self, id_url: &str) -> Result<String, RemoteScrapeError> {
        let attempts = self.max_retries.saturating_add(1);
        let mut attempt = 0_u32;
        loop {
            if attempt > 0 {
                let delay = self
                    .retry_delay
                    .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)));
                tokio::time::sleep(delay).await;
            }
            match self.client.get(id_url).send().await {
                Ok(response) => {
                    let status = response.status();
                    if status == reqwest::StatusCode::NOT_FOUND {
                        return Err(RemoteScrapeError::PageNotFound(id_url.to_owned()));
                    }
                    if status.is_server_error() && attempt + 1 < attempts {
                        attempt += 1;
                        continue;
                    }
                    if !status.is_success() {
                        return Err(RemoteScrapeError::HttpStatus(status.as_u16()));
                    }
                    return response
                        .text()
                        .await
                        .map_err(|err| RemoteScrapeError::Request(err.to_string()));
                }
                Err(_) if attempt + 1 < attempts => {
                    attempt += 1;
                }
                Err(err) => return Err(RemoteScrapeError::Request(err.to_string())),
            }
        }
    }
}
