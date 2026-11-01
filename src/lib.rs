//! `scrpr` is a tiny, composable scraping toolkit.
//!
//! Implement [`RemoteScrapable`] to describe *where* a resource lives (its
//! id-based URL), *which* elements to select (a CSS selector) and *how* to
//! convert them into your own output type. The crate then takes care of
//! fetching pages concurrently with retries and timeouts, or parsing local
//! HTML you already have.
//!
//! # Offline example
//!
//! ```
//! use scrpr::{ElementExt, RemoteScrapable, RemoteScrapeError, scrape_html};
//!
//! struct AppIcon;
//!
//! impl RemoteScrapable for AppIcon {
//!     type Output = String;
//!
//!     fn id_url(&self, id: &str) -> String {
//!         format!("https://store.example.com/app/{id}")
//!     }
//!
//!     fn elem_into(
//!         &self,
//!         elem: scraper::ElementRef<'_>,
//!     ) -> Result<Self::Output, RemoteScrapeError> {
//!         elem.required_attr("src").map(ToOwned::to_owned)
//!     }
//!
//!     fn res_selector(&self) -> &str {
//!         "img.icon"
//!     }
//! }
//!
//! let html = r#"<html><body><img class="icon" src="/icon.png"></body></html>"#;
//! let icon = scrape_html(&AppIcon, html).expect("icon present");
//! assert_eq!(icon, "/icon.png");
//! ```
//!
//! # Remote example
//!
//! ```
//! use scrpr::{ElementExt, RemoteScraper, RemoteScrapable, RemoteScrapeError};
//!
//! struct Title;
//!
//! impl RemoteScrapable for Title {
//!     type Output = String;
//!
//!     fn id_url(&self, id: &str) -> String {
//!         format!("https://example.com/{id}")
//!     }
//!
//!     fn elem_into(
//!         &self,
//!         elem: scraper::ElementRef<'_>,
//!     ) -> Result<Self::Output, RemoteScrapeError> {
//!         Ok(elem.text_content())
//!     }
//!
//!     fn res_selector(&self) -> &str {
//!         "h1"
//!     }
//! }
//!
//! # async fn demo() -> Result<(), RemoteScrapeError> {
//! let scraper = RemoteScraper::builder()
//!     .user_agent("scrpr-example/0.2")
//!     .max_retries(1)
//!     .build()?;
//!
//! // One result per id, always in the input order.
//! let titles = scraper.scrape(&Title, ["a", "b", "c"]).await;
//! # Ok(())
//! # }
//! ```

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![doc(html_root_url = "https://docs.rs/scrpr")]

mod error;
mod scraper;
#[cfg(test)]
mod tests;
mod traits;

pub use crate::error::RemoteScrapeError;
pub use crate::scraper::{
    DEFAULT_CONCURRENCY, DEFAULT_MAX_RETRIES, DEFAULT_RETRY_DELAY, DEFAULT_TIMEOUT, RemoteScraper,
    RemoteScraperBuilder, remote_scrape, scrape_html, scrape_html_all,
};
pub use crate::traits::{ElementExt, RemoteScrapable};
