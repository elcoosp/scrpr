//! The HTTP client: fetching with retries, pacing, caching and
//! bounded-concurrency batch scraping.

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt};
use tokio::sync::Mutex as AsyncMutex;

use crate::error::ScrapeError;
use crate::extract::Extract;
use crate::page::Page;

/// Default maximum number of urls scraped concurrently.
pub const DEFAULT_CONCURRENCY: usize = 8;
/// Default per-request timeout (connect + read + body).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default number of retries per failing request (so 3 total attempts).
pub const DEFAULT_MAX_RETRIES: u32 = 2;
/// Default base delay between retries; the delay doubles on every attempt.
pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(200);
/// Default ceiling for the exponential backoff (and for server-demanded
/// `Retry-After` waits).
pub const DEFAULT_MAX_RETRY_DELAY: Duration = Duration::from_secs(5);
/// Default response body limit, guarding against decompression bombs.
pub const DEFAULT_MAX_BODY_SIZE: usize = 10 * 1024 * 1024;
/// Default lifetime of a cached page.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(300);
/// Default user agent sent with every request.
pub const DEFAULT_USER_AGENT: &str = "scrpr/0.3 (+https://github.com/elcoosp/scrpr)";

fn is_retryable_status(status: u16) -> bool {
    status == 408 || status == 429 || (500..600).contains(&status)
}

/// Pacing gate every request (including retries) must pass.
#[derive(Debug)]
struct RateLimiter {
    min_interval: Duration,
    next_slot: AsyncMutex<Option<tokio::time::Instant>>,
}

impl RateLimiter {
    fn disabled() -> Self {
        Self {
            min_interval: Duration::ZERO,
            next_slot: AsyncMutex::new(None),
        }
    }

    async fn acquire(&self) {
        if self.min_interval.is_zero() {
            return;
        }
        let mut next = self.next_slot.lock().await;
        let now = tokio::time::Instant::now();
        let slot = match *next {
            Some(pending) if pending > now => pending,
            _ => now,
        };
        // Reserve the slot before sleeping so concurrent fetches queue up
        // in arrival order instead of clustering when the sleep ends.
        *next = Some(slot + self.min_interval);
        drop(next);
        tokio::time::sleep_until(slot).await;
    }
}

/// Decoded page body kept in the cache until its TTL expires.
#[derive(Debug)]
struct CachedHtml {
    html: String,
    url: url::Url,
}

/// Optional in-memory page cache (capacity-bounded, TTL-fresh).
///
/// It stores decoded HTML (not parsed documents): `scraper::Html` contains
/// interior mutability and cannot be shared across threads, while a cache
/// hit still skips the whole HTTP round trip.
#[derive(Debug)]
struct PageCache {
    ttl: Duration,
    capacity: usize,
    state: std::sync::Mutex<CacheState>,
}

#[derive(Debug, Default)]
struct CacheState {
    map: HashMap<String, (Instant, Arc<CachedHtml>)>,
    order: VecDeque<String>,
}

impl PageCache {
    fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            ttl,
            capacity,
            state: std::sync::Mutex::new(CacheState::default()),
        }
    }

    fn get(&self, url: &str) -> Option<Arc<CachedHtml>> {
        let mut state = lock_state(&self.state);
        let entry = state
            .map
            .get(url)
            .map(|(stamp, cached)| (stamp.elapsed(), Arc::clone(cached)));
        match entry {
            None => None,
            Some((elapsed, cached)) if elapsed <= self.ttl => {
                // Refresh recency so hot pages survive eviction.
                state.order.retain(|key| key != url);
                state.order.push_back(url.to_owned());
                Some(cached)
            }
            Some(_) => {
                state.map.remove(url);
                state.order.retain(|key| key != url);
                None
            }
        }
    }

    fn insert(&self, url: &str, cached: Arc<CachedHtml>) {
        if self.capacity == 0 {
            return;
        }
        let mut state = lock_state(&self.state);
        if !state.map.contains_key(url) {
            while state.map.len() >= self.capacity {
                match state.order.pop_front() {
                    Some(oldest) => drop(state.map.remove(&oldest)),
                    None => break,
                }
            }
        }
        state.order.retain(|key| key != url);
        state.order.push_back(url.to_owned());
        state.map.insert(url.to_owned(), (Instant::now(), cached));
    }
}

fn lock_state(state: &std::sync::Mutex<CacheState>) -> std::sync::MutexGuard<'_, CacheState> {
    // A panic while holding the lock must not poison the cache forever.
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Reusable, configurable scraping client.
///
/// One instance shares a single pooled [`reqwest::Client`], a pacing gate
/// and (optionally) a page cache; it is cheap to [`Clone`] and safe to share
/// across tasks.
///
/// # Example
///
/// ```
/// use std::time::Duration;
///
/// use scrpr::{Scraper, ScrapeError, field};
///
/// # async fn demo() -> Result<(), ScrapeError> {
/// let scraper = Scraper::builder()
///     .user_agent("my-bot/0.1 (https://my.test)")
///     .timeout(Duration::from_secs(10))
///     .concurrency(4)
///     .max_retries(2)
///     .rate_limit(5)
///     .cache_capacity(256)
///     .build()?;
///
/// let page = scraper.fetch("https://example.net/catalog").await?;
/// let links = field::links().get(&page)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Scraper {
    client: reqwest::Client,
    concurrency: usize,
    max_retries: u32,
    retry_delay: Duration,
    max_retry_delay: Duration,
    respect_retry_after: bool,
    max_body_size: usize,
    cache: Option<Arc<PageCache>>,
    rate_limiter: Arc<RateLimiter>,
}

impl Scraper {
    /// Builds a scraper with all default settings.
    ///
    /// # Errors
    ///
    /// Errors with [`ScrapeError::Client`] when the underlying TLS backend
    /// fails to initialize.
    pub fn new() -> Result<Self, ScrapeError> {
        ScraperBuilder::default().build()
    }

    /// Returns a builder allowing full configuration.
    #[must_use]
    pub fn builder() -> ScraperBuilder {
        ScraperBuilder::default()
    }

    /// Fetches one page, applying retries, pacing and caching.
    ///
    /// The returned [`Page`] borrows nothing from the scraper and the
    /// decoded html is cached, so repeated fetches skip the whole HTTP
    /// round trip.
    ///
    /// # Errors
    ///
    /// Errors with the matching [`ScrapeError`] variant: [`ScrapeError::PageNotFound`]
    /// for 404s, [`ScrapeError::Http`] for other statuses, [`ScrapeError::Timeout`],
    /// [`ScrapeError::Network`] or [`ScrapeError::BodyTooLarge`].
    pub async fn fetch(&self, url: &str) -> Result<Page, ScrapeError> {
        let parsed =
            url::Url::parse(url).map_err(|err| ScrapeError::url_parse(url, err.to_string()))?;
        self.fetch_parsed(parsed).await
    }

    /// Fetches one page and extracts `E` from it.
    ///
    /// # Errors
    ///
    /// Errors from [`Scraper::fetch`] followed by any extraction error.
    pub async fn fetch_as<E: Extract>(&self, url: &str) -> Result<E, ScrapeError> {
        let page = self.fetch(url).await?;
        E::extract(&page)
    }

    /// Scrapes every url concurrently and returns the results in the input
    /// order (one entry per url, including url-parse failures).
    ///
    /// Concurrent fetches are capped at the configured concurrency level.
    ///
    /// # Example
    ///
    /// ```
    /// use scrpr::{Extract, Page, ScrapeError, Scraper, field};
    ///
    /// struct Title(String);
    ///
    /// impl Extract for Title {
    ///     fn extract(page: &Page) -> Result<Self, ScrapeError> {
    ///         Ok(Self(field::text("h1").get(page)?))
    ///     }
    /// }
    ///
    /// # async fn demo() {
    /// let scraper = Scraper::new().unwrap();
    /// let urls = ["https://example.net/a", "https://example.net/b"];
    /// let results = scraper.scrape::<Title, _, _>(urls).await;
    /// # }
    /// ```
    pub async fn scrape<E, I, S>(&self, urls: I) -> Vec<Result<E, ScrapeError>>
    where
        E: Extract,
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.scrape_stream(urls)
            .map(|(_, result)| result)
            .collect()
            .await
    }

    /// Scrapes every url concurrently as a [`Stream`] of
    /// `(input position, result)` pairs, in input order, with at most
    /// `concurrency` pages in flight.
    ///
    /// Prefer this over [`Scraper::scrape`] for large batches: results
    /// become usable as soon as they are ready instead of after the whole
    /// batch.
    ///
    /// Drive it with `futures::StreamExt`.
    pub fn scrape_stream<E, I, S>(&self, urls: I) -> ScrapeStream<'_, E>
    where
        E: Extract,
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        ScrapeStream {
            inner: Box::pin(
                ordered_positions(urls)
                    .map(move |(index, parsed)| async move {
                        (index, self.fetch_and_extract::<E>(parsed).await)
                    })
                    .buffered(self.concurrency),
            ),
        }
    }

    async fn fetch_and_extract<E: Extract>(
        &self,
        parsed: Result<url::Url, ScrapeError>,
    ) -> Result<E, ScrapeError> {
        let page = self.fetch_parsed_or_error(parsed).await?;
        E::extract(&page)
    }

    async fn fetch_parsed_or_error(
        &self,
        parsed: Result<url::Url, ScrapeError>,
    ) -> Result<Page, ScrapeError> {
        self.fetch_parsed(parsed?).await
    }

    async fn fetch_parsed(&self, url: url::Url) -> Result<Page, ScrapeError> {
        let key = url.to_string();
        if let Some(cache) = &self.cache
            && let Some(cached) = cache.get(&key)
        {
            tracing::debug!(url = %key, "page cache hit");
            return Ok(Page::with_url(&cached.html, cached.url.clone()));
        }
        let (html, final_url) = self.fetch_uncached(&url).await?;
        if let Some(cache) = &self.cache {
            cache.insert(
                &key,
                Arc::new(CachedHtml {
                    html: html.clone(),
                    url: final_url.clone(),
                }),
            );
        }
        Ok(Page::with_url(&html, final_url))
    }

    async fn fetch_uncached(&self, url: &url::Url) -> Result<(String, url::Url), ScrapeError> {
        let url_string = url.to_string();
        let attempts = self.max_retries.saturating_add(1);
        let mut last_error: Option<ScrapeError> = None;
        let mut retry_after: Option<Duration> = None;

        for attempt in 0..attempts {
            self.rate_limiter.acquire().await;
            if attempt > 0 {
                let backoff = self
                    .retry_delay
                    .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)));
                let delay = match retry_after {
                    Some(server_demand) if server_demand > backoff => server_demand,
                    _ => backoff,
                }
                .min(self.max_retry_delay);
                tracing::debug!(url = %url_string, attempt, ?delay, "retrying request");
                tokio::time::sleep(delay).await;
            }

            let response = match self.client.get(url.clone()).send().await {
                Ok(response) => response,
                Err(err) => {
                    let mapped = map_request_error(&err, &url_string);
                    if attempt + 1 < attempts {
                        last_error = Some(mapped);
                        continue;
                    }
                    return Err(mapped);
                }
            };

            let status = response.status().as_u16();
            if status == 404 {
                return Err(ScrapeError::PageNotFound { url: url_string });
            }
            if !is_success(status) {
                if is_retryable_status(status) && attempt + 1 < attempts {
                    retry_after = if self.respect_retry_after {
                        parse_retry_after(response.headers(), &url_string)
                    } else {
                        None
                    };
                    last_error = Some(ScrapeError::Http {
                        status,
                        url: url_string.clone(),
                    });
                    continue;
                }
                return Err(ScrapeError::Http {
                    status,
                    url: url_string,
                });
            }

            // The final url (after redirects) is the correct base for
            // resolving relative links on the page.
            let final_url = response.url().clone();
            let (bytes, content_type) =
                read_body_capped(response, self.max_body_size, &url_string).await?;
            let text = decode_html(&bytes, content_type.as_deref());
            return Ok((text, final_url));
        }
        Err(last_error.unwrap_or_else(|| ScrapeError::Network {
            url: url_string,
            message: "retry attempts exhausted".to_owned(),
        }))
    }
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// Enumerates the input urls into `(position, parsed url)` pairs.
///
/// Url-parse failures are carried as per-item errors so one bad url never
/// aborts a batch. Returns a concrete iterator-backed stream so the batch
/// apis carry no generic parameter into their returned types.
fn ordered_positions<I, S>(
    urls: I,
) -> futures::stream::Iter<std::vec::IntoIter<(usize, Result<url::Url, ScrapeError>)>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    futures::stream::iter(
        urls.into_iter()
            .enumerate()
            .map(|(index, url)| {
                let parsed = url::Url::parse(url.as_ref())
                    .map_err(|err| ScrapeError::url_parse(url.as_ref(), err.to_string()));
                (index, parsed)
            })
            .collect::<Vec<_>>(),
    )
}

fn map_request_error(err: &reqwest::Error, url: &str) -> ScrapeError {
    if err.is_timeout() {
        ScrapeError::Timeout {
            url: url.to_owned(),
        }
    } else if err.is_redirect() {
        ScrapeError::Network {
            url: url.to_owned(),
            message: "too many redirects".to_owned(),
        }
    } else {
        ScrapeError::Network {
            url: url.to_owned(),
            message: err.to_string(),
        }
    }
}

/// Parses the seconds form of `Retry-After` (the HTTP-date form is ignored).
fn parse_retry_after(headers: &reqwest::header::HeaderMap, url: &str) -> Option<Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let seconds: u64 = raw.trim().parse().ok()?;
    if seconds > 3600 {
        tracing::debug!(url, seconds, "clamping unreasonable Retry-After to 1h");
        return Some(Duration::from_secs(3600));
    }
    Some(Duration::from_secs(seconds))
}

async fn read_body_capped(
    mut response: reqwest::Response,
    limit: usize,
    url: &str,
) -> Result<(Vec<u8>, Option<String>), ScrapeError> {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    if let Some(declared) = response.content_length()
        && declared > limit as u64
    {
        return Err(ScrapeError::BodyTooLarge {
            url: url.to_owned(),
            limit,
        });
    }
    let mut body = Vec::new();
    if let Some(declared) = response.content_length() {
        // After the cap check above, `declared` always fits into `usize`.
        body.reserve(usize::try_from(declared.min(limit as u64)).unwrap_or(limit));
    }
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| map_request_error(&err, url))?
    {
        if body.len() + chunk.len() > limit {
            return Err(ScrapeError::BodyTooLarge {
                url: url.to_owned(),
                limit,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, content_type))
}

/// Decodes raw response bytes into text, honoring the `Content-Type`
/// charset parameter (UTF-8 by default, BOM-aware).
fn decode_html(bytes: &[u8], content_type: Option<&str>) -> String {
    let charset = content_type.and_then(|header| {
        header.split(';').find_map(|part| {
            let trimmed = part.trim();
            trimmed
                .strip_prefix("charset=")
                .map(|value| value.trim_matches('"').to_ascii_lowercase())
        })
    });
    let encoding = charset
        .as_deref()
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

/// Type-erased, sendable, ordered scrape stream.
type BoxedScrapeStream<'a, E> =
    Pin<Box<dyn Stream<Item = (usize, Result<E, ScrapeError>)> + Send + 'a>>;

/// Ordered [`Stream`] produced by [`Scraper::scrape_stream`], yielding
/// `(input position, result)` pairs.
///
/// Drive it with `futures::StreamExt`.
pub struct ScrapeStream<'a, E>
where
    E: 'a,
{
    inner: BoxedScrapeStream<'a, E>,
}

impl<E> Stream for ScrapeStream<'_, E> {
    type Item = (usize, Result<E, ScrapeError>);

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Configures and builds a [`Scraper`].
///
/// # Example
///
/// ```
/// use std::time::Duration;
///
/// use scrpr::Scraper;
///
/// let scraper = Scraper::builder()
///     .user_agent("my-crawler/0.1")
///     .timeout(Duration::from_secs(5))
///     .connect_timeout(Duration::from_secs(2))
///     .build()
///     .expect("valid configuration");
/// ```
#[derive(Debug)]
pub struct ScraperBuilder {
    client: reqwest::ClientBuilder,
    default_headers: reqwest::header::HeaderMap,
    concurrency: usize,
    max_retries: u32,
    retry_delay: Duration,
    max_retry_delay: Duration,
    respect_retry_after: bool,
    max_body_size: usize,
    cache_capacity: usize,
    cache_ttl: Duration,
    min_request_interval: Duration,
}

impl Default for ScraperBuilder {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(DEFAULT_USER_AGENT)
                .timeout(DEFAULT_TIMEOUT),
            default_headers: reqwest::header::HeaderMap::new(),
            concurrency: DEFAULT_CONCURRENCY,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_delay: DEFAULT_RETRY_DELAY,
            max_retry_delay: DEFAULT_MAX_RETRY_DELAY,
            respect_retry_after: true,
            max_body_size: DEFAULT_MAX_BODY_SIZE,
            cache_capacity: 0,
            cache_ttl: DEFAULT_CACHE_TTL,
            min_request_interval: Duration::ZERO,
        }
    }
}

impl ScraperBuilder {
    /// Sets the `User-Agent` header sent with every request.
    #[must_use]
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.client = self.client.user_agent(user_agent.into());
        self
    }

    /// Adds a default header sent with every request. Call repeatedly to
    /// add several headers.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            self.default_headers.insert(name, value);
        }
        self
    }

    /// Sets the timeout applied to each request (connect + read + body).
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.client = self.client.timeout(timeout);
        self
    }

    /// Sets the timeout for establishing each connection.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.client = self.client.connect_timeout(timeout);
        self
    }

    /// Sets the timeout for receiving data between body chunks.
    #[must_use]
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.client = self.client.read_timeout(timeout);
        self
    }

    /// Sets the maximum number of urls fetched concurrently.
    ///
    /// Values below 1 are clamped to 1.
    #[must_use]
    pub const fn concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = if concurrency < 1 { 1 } else { concurrency };
        self
    }

    /// Sets how many times a failing request (network error, timeout, or
    /// HTTP 408/429/5xx) is retried before surfacing the error.
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

    /// Sets the ceiling applied to backoff delays *and* to server-demanded
    /// `Retry-After` waits.
    #[must_use]
    pub const fn max_retry_delay(mut self, max_retry_delay: Duration) -> Self {
        self.max_retry_delay = max_retry_delay;
        self
    }

    /// Whether a 429 response's `Retry-After` header (seconds form) delays
    /// the retry. Defaults to `true`.
    #[must_use]
    pub const fn respect_retry_after(mut self, respect: bool) -> Self {
        self.respect_retry_after = respect;
        self
    }

    /// Caps the response body size in bytes (default 10 MiB).
    #[must_use]
    pub const fn max_body_size(mut self, max_body_size: usize) -> Self {
        self.max_body_size = max_body_size;
        self
    }

    /// Enables the page cache with the given entry capacity.
    ///
    /// The cache is disabled by default; entries expire after
    /// [`ScraperBuilder::cache_ttl`] and only successful fetches are stored.
    #[must_use]
    pub const fn cache_capacity(mut self, capacity: usize) -> Self {
        self.cache_capacity = capacity;
        self
    }

    /// Sets the lifetime of cached pages (default 5 minutes).
    #[must_use]
    pub const fn cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Paces the client at `max_requests_per_second` requests per second
    /// across all concurrent fetches.
    #[must_use]
    pub fn rate_limit(mut self, max_requests_per_second: u32) -> Self {
        self.min_request_interval = if max_requests_per_second == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs(1) / max_requests_per_second.max(1)
        };
        self
    }

    /// Sets the minimum interval between two request starts (including
    /// retries). Overrides [`ScraperBuilder::rate_limit`].
    #[must_use]
    pub const fn min_request_interval(mut self, interval: Duration) -> Self {
        self.min_request_interval = interval;
        self
    }

    /// Enables or disables redirect following (enabled by default).
    #[must_use]
    pub fn follow_redirects(mut self, follow: bool) -> Self {
        self.client = self.client.redirect(if follow {
            reqwest::redirect::Policy::default()
        } else {
            reqwest::redirect::Policy::none()
        });
        self
    }

    /// Limits how many redirects are followed per request.
    #[must_use]
    pub fn max_redirects(mut self, max: usize) -> Self {
        self.client = self
            .client
            .redirect(reqwest::redirect::Policy::limited(max));
        self
    }

    /// Enables the client-side cookie store (disabled by default).
    #[must_use]
    pub fn cookie_store(mut self, enable: bool) -> Self {
        self.client = self.client.cookie_store(enable);
        self
    }

    /// Routes every request through the given proxy.
    #[must_use]
    pub fn proxy(mut self, proxy: reqwest::Proxy) -> Self {
        self.client = self.client.proxy(proxy);
        self
    }

    /// Accepts invalid TLS certificates (self-signed hosts, stale servers).
    #[must_use]
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.client = self.client.danger_accept_invalid_certs(accept);
        self
    }

    /// Builds the configured [`Scraper`].
    ///
    /// # Errors
    ///
    /// Errors with [`ScrapeError::Client`] when the underlying
    /// [`reqwest::Client`] cannot be built (for example an invalid user
    /// agent header value).
    pub fn build(self) -> Result<Scraper, ScrapeError> {
        let client = self
            .client
            .default_headers(self.default_headers)
            .build()
            .map_err(|err| ScrapeError::Client(err.to_string()))?;
        let rate_limiter = if self.min_request_interval.is_zero() {
            RateLimiter::disabled()
        } else {
            RateLimiter {
                min_interval: self.min_request_interval,
                next_slot: AsyncMutex::new(None),
            }
        };
        Ok(Scraper {
            client,
            concurrency: self.concurrency,
            max_retries: self.max_retries,
            retry_delay: self.retry_delay,
            max_retry_delay: self.max_retry_delay,
            respect_retry_after: self.respect_retry_after,
            max_body_size: self.max_body_size,
            cache: if self.cache_capacity == 0 {
                None
            } else {
                Some(Arc::new(PageCache::new(
                    self.cache_capacity,
                    self.cache_ttl,
                )))
            },
            rate_limiter: Arc::new(rate_limiter),
        })
    }
}

/// Fetches one page with a throwaway default [`Scraper`].
///
/// Handy for one-off scripts; reuse a [`Scraper`] (which pools connections)
/// when fetching more than a page or two.
///
/// # Errors
///
/// Same as [`Scraper::fetch`].
pub async fn fetch(url: &str) -> Result<Page, ScrapeError> {
    Scraper::new()?.fetch(url).await
}

/// Fetches one page with a throwaway default [`Scraper`] and extracts `E`.
///
/// # Errors
///
/// Same as [`Scraper::fetch_as`].
pub async fn fetch_as<E: Extract>(url: &str) -> Result<E, ScrapeError> {
    Scraper::new()?.fetch_as::<E>(url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_statuses_are_classified() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(599));
        assert!(!is_retryable_status(404));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(418));
    }

    #[test]
    fn success_window_is_2xx() {
        assert!(is_success(200));
        assert!(is_success(299));
        assert!(!is_success(199));
        assert!(!is_success(300));
    }

    #[test]
    fn retry_after_parses_seconds_and_ignores_dates() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "12".parse().unwrap());
        assert_eq!(
            parse_retry_after(&headers, "u"),
            Some(Duration::from_secs(12))
        );

        headers.insert(reqwest::header::RETRY_AFTER, " 5 ".parse().unwrap());
        assert_eq!(
            parse_retry_after(&headers, "u"),
            Some(Duration::from_secs(5))
        );

        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(parse_retry_after(&headers, "u"), None);

        headers.insert(reqwest::header::RETRY_AFTER, "99999".parse().unwrap());
        assert_eq!(
            parse_retry_after(&headers, "u"),
            Some(Duration::from_secs(3600))
        );
    }

    #[test]
    fn decode_html_honors_charset_and_defaults_to_utf8() {
        let latin = vec![0xC9_u8, b'!'];
        assert_eq!(
            decode_html(&latin, Some("text/html; charset=iso-8859-1")),
            "É!"
        );
        assert_eq!(decode_html("héllo".as_bytes(), None), "héllo");
        assert_eq!(
            decode_html("héllo".as_bytes(), Some("text/html; charset=UTF-8")),
            "héllo"
        );
        assert_eq!(
            decode_html("x".as_bytes(), Some("text/html; charset=\"utf-8\"")),
            "x"
        );
        assert_eq!(
            decode_html("x".as_bytes(), Some("text/html; charset=unknown-charset")),
            "x"
        );
    }

    #[test]
    fn cache_respects_capacity_and_ttl() {
        let cached = Arc::new(CachedHtml {
            html: "<html></html>".to_owned(),
            url: url::Url::parse("https://x.test").unwrap(),
        });
        let cache = PageCache::new(1, Duration::from_millis(30));

        cache.insert("a", Arc::clone(&cached));
        assert!(cache.get("a").is_some());
        cache.insert("b", Arc::clone(&cached));
        assert!(
            cache.get("a").is_none(),
            "capacity 1 must evict the oldest entry"
        );
        assert!(cache.get("b").is_some());

        std::thread::sleep(Duration::from_millis(50));
        assert!(cache.get("b").is_none(), "entries expire after the ttl");
    }

    #[test]
    fn cache_get_refreshes_recency() {
        let cached = Arc::new(CachedHtml {
            html: "<html></html>".to_owned(),
            url: url::Url::parse("https://x.test").unwrap(),
        });
        let cache = PageCache::new(2, Duration::from_secs(60));
        cache.insert("a", Arc::clone(&cached));
        cache.insert("b", Arc::clone(&cached));
        assert!(cache.get("a").is_some());
        cache.insert("c", Arc::clone(&cached));
        assert!(cache.get("a").is_some(), "recently used entry survives");
        assert!(
            cache.get("b").is_none(),
            "least recently used entry is evicted"
        );
    }

    #[test]
    fn disabled_rate_limiter_is_a_no_op() {
        let limiter = RateLimiter::disabled();
        assert!(limiter.min_interval.is_zero());
    }

    #[tokio::test]
    async fn rate_limiter_paces_acquisitions() {
        let limiter = Arc::new(RateLimiter {
            min_interval: Duration::from_millis(40),
            next_slot: AsyncMutex::new(None),
        });
        let start = Instant::now();
        let mut handles = Vec::new();
        for _ in 0..3 {
            let limiter = Arc::clone(&limiter);
            handles.push(tokio::spawn(async move { limiter.acquire().await }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        // Two forced gaps of 40ms between three acquisitions.
        assert!(start.elapsed() >= Duration::from_millis(70));
    }

    #[test]
    fn builder_validates_configuration() {
        let built = Scraper::builder().user_agent("bad\nheader").build();
        assert!(matches!(built, Err(ScrapeError::Client(_))));
    }

    #[test]
    fn builder_setters_compose() {
        let proxy = reqwest::Proxy::all("http://127.0.0.1:1").unwrap();
        let scraper = Scraper::builder()
            .timeout(Duration::from_secs(1))
            .connect_timeout(Duration::from_secs(1))
            .read_timeout(Duration::from_secs(1))
            .concurrency(3)
            .max_retries(5)
            .retry_delay(Duration::from_millis(5))
            .max_retry_delay(Duration::from_millis(50))
            .respect_retry_after(false)
            .max_body_size(1024)
            .cache_capacity(8)
            .cache_ttl(Duration::from_secs(1))
            .rate_limit(10)
            .min_request_interval(Duration::from_millis(1))
            .follow_redirects(false)
            .max_redirects(2)
            .cookie_store(true)
            .proxy(proxy)
            .danger_accept_invalid_certs(true)
            .header("x-test", "1")
            .build()
            .unwrap();
        let _ = format!("{scraper:?}");
    }

    #[test]
    fn rate_limit_zero_disables_pacing() {
        let scraper = Scraper::builder().rate_limit(0).build().unwrap();
        assert!(scraper.rate_limiter.min_interval.is_zero());
        assert!(scraper.cache.is_none());
    }

    #[test]
    fn default_constants_are_sane() {
        assert_eq!(DEFAULT_CONCURRENCY, 8);
        assert_eq!(DEFAULT_MAX_RETRIES, 2);
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
        assert_eq!(DEFAULT_RETRY_DELAY, Duration::from_millis(200));
        assert_eq!(DEFAULT_MAX_RETRY_DELAY, Duration::from_secs(5));
        assert_eq!(DEFAULT_MAX_BODY_SIZE, 10 * 1024 * 1024);
        assert_eq!(DEFAULT_CACHE_TTL, Duration::from_secs(300));
        assert!(DEFAULT_USER_AGENT.contains("scrpr"));
    }

    #[test]
    fn page_cache_debug_is_available() {
        let cache = PageCache::new(2, Duration::from_secs(1));
        assert!(format!("{cache:?}").contains("PageCache"));
    }
}
