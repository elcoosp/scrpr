<div align="center">
  <img src="docs/logo.png" alt="Scrpr Logo" width="300"/>
  <p>
    <strong>Typed, declarative web scraping for Rust.</strong><br/>
    Describe which elements to select and how to convert them into your own
    types; <code>scrpr</code> fetches pages concurrently with retries, rate
    limiting and caching — or parses HTML you already have.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Rust-1.88%2B-000000?style=flat-square&logo=rust" alt="Rust"/>
    <img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License MIT"/>
    <img src="https://img.shields.io/badge/Tests-105%20passing-brightgreen?style=flat-square" alt="Tests"/>
    <img src="https://img.shields.io/badge/CI-fmt%20%7C%20clippy%20pedantic%20%7C%20MSRV-6A0DAD?style=flat-square" alt="CI"/>
    <img src="https://img.shields.io/badge/Crates.io-scrpr-orange?style=flat-square&logo=rust" alt="Crates.io"/>
    <img src="https://img.shields.io/badge/Docs.rs-API%20Reference-4B32C3?style=flat-square&logo=docsdotrs" alt="Docs.rs"/>
  </p>
</div>

---

## About the project

`scrpr` is a tiny, composable scraping toolkit: **typed extraction for HTML
pages, with a polite HTTP client attached**.

- Describe *how to pull fields out of a page* with the `field` extractor DSL
  or the serde-style `Extract` trait.
- The crate fetches pages concurrently (bounded, ordered, retried, paced,
  optionally cached) or parses HTML you already have — every failure is a
  typed, serializable, context-rich error.

## Features

- **Typed extraction**: implement `Extract` once for your domain type, scrape
  it offline (`scrape_html`), single (`fetch_as`) or in batches (`scrape`,
  `scrape_stream`).
- **Composable field DSL**: `field::text`, `field::attr`, `field::attr_url`,
  `field::links`, `field::json_ld`, ... plus `map`, `try_map`, `parse::<F>()`,
  `optional()`, `or()` transforms and tuple extraction for up to 12 fields.
- **Bounded concurrency**: up to `N` requests in flight (default 8), results
  always returned in input order; `scrape_stream` yields `(position, result)`
  pairs as pages complete.
- **Politeness**: exponential-backoff retries (capped) on network errors and
  HTTP 408/429/5xx, `Retry-After` honored by default, per-client rate
  limiting (`rate_limit` / `min_request_interval`), identifying default
  `User-Agent`.
- **Robustness**: total/connect/read timeouts, response body size cap
  (decompression-bomb guard), charset decoding (UTF-8 default, ISO-8859-1
  & friends via the `Content-Type` header), redirect and cookie controls,
  proxy support.
- **Optional page cache**: TTL + capacity bounded, deduplicates repeated urls
  inside a batch, shared across clones of the same `Scraper`.
- **Fast by construction**: one pooled `reqwest::Client` per scraper,
  process-wide CSS selector cache, gzip/brotli/zstd/deflate auto-decompression.
- **Serializable errors**: every `ScrapeError` variant carries context (url,
  status, selector, attribute...) and round-trips through serde.
- **100% offline testability**: the whole test suite (89 integration tests +
  16 doctests) runs against a hand-rolled local HTTP server; no network in CI.

## Getting started

### Installation

```bash
cargo add scrpr
```

### Quick tour

```rust
use scrpr::{field, Extract, Page, ScrapeError};

struct Product {
    title: String,
    price: f64,
}

impl Extract for Product {
    fn extract(page: &Page) -> Result<Self, ScrapeError> {
        let (title, price) = scrpr::extract(
            page,
            (
                field::text("h1.name"),
                field::text("span.price").parse::<f64>(),
            ),
        )?;
        Ok(Self { title, price })
    }
}

// Offline: parse HTML you already have.
let html = r#"<h1 class="name">Chair</h1><span class="price">49.90</span>"#;
let product = scrpr::scrape_html::<Product>(html)?;
assert_eq!(product.title, "Chair");
```

### Remote scraping

```rust
use std::time::Duration;

use scrpr::{Extract, Page, ScrapeError, Scraper, field};

struct Title(String);

impl Extract for Title {
    fn extract(page: &Page) -> Result<Self, ScrapeError> {
        Ok(Self(field::text("h1").get(page)?))
    }
}

async fn demo() -> Result<(), ScrapeError> {
    let scraper = Scraper::builder()
        .user_agent("my-bot/0.1 (https://my.test)")
        .timeout(Duration::from_secs(10))
        .concurrency(8)
        .max_retries(2)
        .rate_limit(10)       // max 10 requests/second, all fetches combined
        .cache_capacity(256)  // re-runs hit the cache instead of the network
        .build()?;

    // One page, extracted.
    let title = scraper.fetch_as::<Title>("https://example.net/a").await?;

    // A batch: one typed result per url, in input order.
    let urls = ["https://example.net/a", "https://example.net/b"];
    let results = scraper.scrape::<Title, _, _>(urls).await;

    // A stream: (position, result) pairs as soon as each page completes.
    let mut stream = scraper.scrape_stream::<Title, _, _>(urls);
    while let Some((position, result)) = futures::StreamExt::next(&mut stream).await {
        println!("{position}: {}", result.is_ok());
    }
    Ok(())
}
```

### Ad-hoc fields without a struct

```rust
use scrpr::{field, Page};

let page = Page::parse_with_base(
    r#"<img class="icon" src="/icon.png"><a href="/next">next</a>"#,
    "https://example.net/catalog",
)?;

let icon = field::attr_url("img.icon", "src").get(&page)?;
assert_eq!(icon.as_str(), "https://example.net/icon.png");

let (title, links) = scrpr::extract(&page, (field::title(), field::links()))?;
```

`Page` also exposes `select`, `select_first`, `links`, `title`, `meta`,
`meta_property`, `scripts` and `json_ld` (behind the default `json` feature)
for full-power, manual extraction.

## Client configuration

| Builder method | Default | Controls |
|---|---|---|
| `user_agent` | `scrpr/0.3 (+...)` | identifying `User-Agent` header |
| `header` | — | extra default headers |
| `timeout` / `connect_timeout` / `read_timeout` | 30 s / — / — | request timeouts |
| `concurrency` | 8 | max pages in flight |
| `max_retries` | 2 | retries for network errors + 408/429/5xx |
| `retry_delay` / `max_retry_delay` | 200 ms / 5 s | exponential backoff bounds |
| `respect_retry_after` | `true` | honor `Retry-After` on 429 |
| `rate_limit` / `min_request_interval` | off | client-wide pacing |
| `max_body_size` | 10 MiB | decompression-bomb guard |
| `cache_capacity` / `cache_ttl` | off / 5 min | in-memory page cache |
| `follow_redirects` / `max_redirects` | on / 10 | redirect policy |
| `cookie_store` | off | client-side cookies |
| `proxy`, `danger_accept_invalid_certs` | — | network escape hatches |

## Migrating from 0.2 (breaking)

0.3 replaces the `RemoteScrapable` trait with two decoupled concepts —
`Extract` (conversion) and plain url iterators (source):

| 0.2 | 0.3 |
|-----|-----|
| `impl RemoteScrapable` (`id_url` + `res_selector` + `elem_into`) | `impl Extract` (`fn extract(page: &Page)`) |
| `RemoteScraper` / `RemoteScraperBuilder` | `Scraper` / `ScraperBuilder` |
| `scraper.scrape(&resource, ids)` | `scraper.scrape::<E, _, _>(urls)` |
| `remote_scrape(&resource, ids)` | `Scraper::new()?.scrape::<E, _, _>(..)` or free `fetch_as` |
| `scrape_html(&resource, html)` | `scrape_html::<E>(html)` |
| `scrape_html_all(&resource, html)` | `field::text_all(sel).get(&page)` / `page.select(sel)` |
| `RemoteScrapeError::HttpStatus(u16)` | `ScrapeError::Http { status, url }` |
| `ElementExt::required_attr(name)` | kept; the error now carries selector context |

The design rationale (including why the id concept was dropped and why
`Page` is intentionally not `Send`) lives in
[docs/BRAINSTORM.md](docs/BRAINSTORM.md).

## Development

```bash
cargo fmt --all --check            # formatting
cargo clippy --all-targets -- -D warnings   # pedantic lints
cargo test                         # unit + integration + doctests
```

---

## Acknowledgments

- [Scraper crate](https://docs.rs/scraper) — html5ever-backed CSS selection.
- [reqwest](https://docs.rs/reqwest) — the HTTP client underneath.
