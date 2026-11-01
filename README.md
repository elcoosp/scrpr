<div align="center">
  <img src="docs/logo.png" alt="Scrpr Logo" width="300"/>
  <p>
    <strong>A tiny, composable scraping toolkit written in Rust.</strong><br/>
    Describe where a resource lives, which elements to select, and how to convert them. Fetches pages concurrently with retries, timeouts, and bounded concurrency, or parses local HTML you already have.
  </p>
  <p>
    <img src="https://img.shields.io/badge/Rust-1.70%2B%20%7C%202021-000000?style=flat-square&logo=rust" alt="Rust"/>
    <img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License MIT"/>
    <img src="https://img.shields.io/badge/Coverage-100%25-brightgreen?style=flat-square&logo=codecov" alt="Coverage"/>
    <img src="https://img.shields.io/badge/Features-Concurrent%20%7C%20Retries%20%7C%20Offline-6A0DAD?style=flat-square" alt="Features"/>
    <img src="https://img.shields.io/badge/Crates.io-scrpr-orange?style=flat-square&logo=rust" alt="Crates.io"/>
    <img src="https://img.shields.io/badge/Docs.rs-API%20Reference-4B32C3?style=flat-square&logo=docsdotrs" alt="Docs.rs"/>
  </p>
</div>

---

## About the Project

`scrpr` is a tiny, composable scraping toolkit: describe *where* a resource lives (its id-based URL), *which* elements to select (a CSS selector) and *how* to convert them into your own output type. The crate takes care of fetching pages concurrently with retries and timeouts, or parsing local HTML you already have.

---

## Features

- **Declarative scraping**: Implement the `RemoteScrapable` trait once (URL template + CSS selector + conversion) and scrape any number of IDs.
- **Bounded concurrency**: Up to `N` requests in flight (default 8), with results always returned in input order.
- **Retries with exponential backoff**: Automatically retries on network errors and HTTP 5xx, configurable via `max_retries` and `retry_delay`.
- **Timeouts and custom `User-Agent`**: Easily configurable through the `RemoteScraper::builder()`.
- **Offline scraping**: `scrape_html` and `scrape_html_all` parse already-downloaded pages without network access, selecting the first or every match.
- **Ergonomic element helpers**: `ElementExt::required_attr` and `ElementExt::text_content` avoid `unwrap`-prone extraction code.
- **Serializable errors**: `RemoteScrapeError` is `Clone + Serialize + Deserialize`, so results can be stored or sent across the network.
- **100% test coverage**: Enforced in CI with `cargo-llvm-cov`.

---

## Getting Started

### Installation

Add the crate to your project using Cargo:

```bash
cargo add scrpr
```

### Usage

The two main entry points are `RemoteScraper` for remote fetching (with timeouts, user agent, retries, concurrency) and `scrape_html` / `scrape_html_all` for local parsing.

> [!NOTE]
> Every failure mode is a typed variant of `RemoteScrapeError`: `Request`, `UrlParse`, `NoElementMatch`, `PageNotFound`, `HttpStatus`, `SelectorParse`, or `MissingAttribute`. See the crate documentation on [docs.rs](https://docs.rs/scrpr) for runnable examples.

#### Example

```rust
use scrpr::{ElementExt, RemoteScrapable, RemoteScrapeError, scrape_html};

struct AppIcon;

impl RemoteScrapable for AppIcon {
    type Output = String;

    fn id_url(&self, id: &str) -> String {
        format!("https://store.example.com/app/{id}")
    }

    fn elem_into(
        &self,
        elem: scraper::ElementRef<'_>,
    ) -> Result<Self::Output, RemoteScrapeError> {
        elem.required_attr("src").map(ToOwned::to_owned)
    }

    fn res_selector(&self) -> &str {
        "img.icon"
    }
}

// Offline: parse HTML you already have.
let html = r#"<html><body><img class="icon" src="/icon.png"></body></html>"#;
let icon = scrape_html(&AppIcon, html)?;

// Remote: fetch IDs concurrently with one result per ID, in input order.
// let icons = RemoteScraper::builder().user_agent("my-bot/0.1").build()?.scrape(&AppIcon, ["id1", "id2"]).await;
```

---

## Roadmap

See the [open issues](https://github.com/elcoosp/scrpr/issues) for a full list of proposed features and known issues.

---

## Acknowledgments

- [Scraper crate](https://docs.rs/scraper/0.21.0/scraper/)
