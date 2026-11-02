//! `scrpr` is a tiny, composable scraping toolkit: **typed extraction for
//! HTML pages, with a polite HTTP client attached**.
//!
//! Describe *how to pull fields out of a page* with the [`field`] DSL or the
//! [`Extract`] trait (serde-style: one impl per domain type), and the crate
//! takes care of fetching pages concurrently — with retries, timeouts, rate
//! limiting, an optional TTL cache and bounded concurrency — or parsing HTML
//! you already have.
//!
//! # Offline extraction
//!
//! ```
//! use scrpr::{field, Page};
//!
//! let page = Page::parse(r#"<html><body><img class="icon" src="/icon.png"></body></html>"#);
//! let icon = field::attr("img.icon", "src").get(&page).unwrap();
//! assert_eq!(icon, "/icon.png");
//! ```
//!
//! # Typed extraction into your own structs
//!
//! ```
//! use scrpr::{field, Extract, Page, ScrapeError};
//!
//! struct Product {
//!     title: String,
//!     price: f64,
//! }
//!
//! impl Extract for Product {
//!     fn extract(page: &Page) -> Result<Self, ScrapeError> {
//!         let (title, price) = scrpr::extract(
//!             page,
//!             (
//!                 field::text("h1.name"),
//!                 field::text("span.price").parse::<f64>(),
//!             ),
//!         )?;
//!         Ok(Self { title, price })
//!     }
//! }
//!
//! let html = r#"<h1 class="name">Chair</h1><span class="price">49.90</span>"#;
//! let product = scrpr::scrape_html::<Product>(html).unwrap();
//! assert_eq!(product.title, "Chair");
//! assert!((product.price - 49.9).abs() < 1e-9);
//! ```
//!
//! # Remote fetching
//!
//! ```
//! # use std::time::Duration;
//! use scrpr::{Scraper, ScrapeError, field};
//!
//! # async fn demo() -> Result<(), ScrapeError> {
//! let scraper = Scraper::builder()
//!     .user_agent("my-bot/0.1")
//!     .timeout(Duration::from_secs(10))
//!     .concurrency(8)
//!     .max_retries(2)
//!     .rate_limit(10)          // politeness: max 10 requests/second
//!     .cache_capacity(256)     // dev-friendliness: re-runs hit the cache
//!     .build()?;
//!
//! let page = scraper.fetch("https://example.net/catalog").await?;
//! let links = field::links().get(&page)?;
//! # Ok(())
//! # }
//! ```
//!
//! # Batch and streaming
//!
//! ```
//! use scrpr::{Extract, Page, ScrapeError, Scraper, field};
//!
//! struct Title(String);
//!
//! impl Extract for Title {
//!     fn extract(page: &Page) -> Result<Self, ScrapeError> {
//!         Ok(Self(field::text("h1").get(page)?))
//!     }
//! }
//!
//! # async fn demo() {
//! let scraper = Scraper::new().unwrap();
//! let urls = (1..=100).map(|i| format!("https://example.net/page/{i}"));
//!
//! // Collect everything: Vec<Result<Title>, _> in input order.
//! let titles = scraper.scrape::<Title, _, _>(urls).await;
//!
//! // ...or consume as they complete: Stream<Item = (usize, Result<Title>)>.
//! # let urls = (1..=100).map(|i| format!("https://example.net/page/{i}"));
//! let mut stream = scraper.scrape_stream::<Title, _, _>(urls);
//! // while let Some((position, result)) = stream.next().await { ... }
//! # }
//! ```

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![doc(html_root_url = "https://docs.rs/scrpr")]

mod client;
mod error;
mod extract;
mod page;
mod selector;
#[cfg(test)]
mod tests;

pub use crate::client::{
    DEFAULT_CACHE_TTL, DEFAULT_CONCURRENCY, DEFAULT_MAX_BODY_SIZE, DEFAULT_MAX_RETRIES,
    DEFAULT_MAX_RETRY_DELAY, DEFAULT_RETRY_DELAY, DEFAULT_TIMEOUT, DEFAULT_USER_AGENT,
    ScrapeStream, Scraper, ScraperBuilder, fetch, fetch_as,
};
pub use crate::error::ScrapeError;
pub use crate::extract::{
    Extract, Extractor, FieldSet, extract, extract_from, field, scrape_html, scrape_html_with,
};
pub use crate::page::{ElementExt, Page};
pub use scraper::{ElementRef, Html, Selector};

/// One-stop import for the everyday API.
///
/// ```
/// use scrpr::prelude::*;
///
/// let page = Page::parse(r#"<h1>Hi</h1>"#);
/// assert_eq!(field::text("h1").get(&page).unwrap(), "Hi");
/// ```
pub mod prelude {
    pub use crate::client::{Scraper, ScraperBuilder};
    pub use crate::error::ScrapeError;
    pub use crate::extract::{
        Extract, Extractor, FieldSet, extract, extract_from, field, scrape_html, scrape_html_with,
    };
    pub use crate::page::{ElementExt, Page};
}
