//! Error types for the `scrpr` crate.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Every failure that can occur while scraping a remote or local document.
///
/// The type is [`Clone`], [`PartialEq`], [`Eq`], [`Serialize`] and
/// [`Deserialize`] so that scrape results can be stored, compared and sent
/// across threads or the network without losing the failure reason.
#[derive(Debug, Error, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoteScrapeError {
    /// The HTTP request failed (network error, timeout, aborted body...).
    #[error("request failed: {0}")]
    Request(String),
    /// A URL could not be parsed.
    #[error("url parse error: {0}")]
    UrlParse(String),
    /// No element matched the resource selector.
    #[error("no element matched the selector")]
    NoElementMatch,
    /// The remote resource answered with HTTP 404 Not Found.
    #[error("page not found: {0}")]
    PageNotFound(String),
    /// The remote resource answered with another non-success status code.
    #[error("server returned error status: {0}")]
    HttpStatus(u16),
    /// The resource selector is not valid CSS.
    #[error("selector parse error: {0}")]
    SelectorParse(String),
    /// A required HTML attribute was absent from the matched element.
    #[error("missing attribute: {0}")]
    MissingAttribute(String),
}

impl From<url::ParseError> for RemoteScrapeError {
    fn from(err: url::ParseError) -> Self {
        Self::UrlParse(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_explicit() {
        assert_eq!(
            RemoteScrapeError::Request("boom".to_owned()).to_string(),
            "request failed: boom"
        );
        assert_eq!(
            RemoteScrapeError::UrlParse("oops".to_owned()).to_string(),
            "url parse error: oops"
        );
        assert_eq!(
            RemoteScrapeError::NoElementMatch.to_string(),
            "no element matched the selector"
        );
        assert_eq!(
            RemoteScrapeError::PageNotFound("/x".to_owned()).to_string(),
            "page not found: /x"
        );
        assert_eq!(
            RemoteScrapeError::HttpStatus(403).to_string(),
            "server returned error status: 403"
        );
        assert_eq!(
            RemoteScrapeError::SelectorParse("bad".to_owned()).to_string(),
            "selector parse error: bad"
        );
        assert_eq!(
            RemoteScrapeError::MissingAttribute("src".to_owned()).to_string(),
            "missing attribute: src"
        );
    }

    #[test]
    fn url_parse_error_converts() {
        let err: RemoteScrapeError = url::Url::parse("::nope::").unwrap_err().into();
        assert_eq!(
            err,
            RemoteScrapeError::UrlParse("relative URL without a base".to_owned())
        );
    }

    #[test]
    fn error_serializes_and_deserializes() {
        let err = RemoteScrapeError::PageNotFound("https://example.net".to_owned());
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"PageNotFound":"https://example.net"}"#);
        assert_eq!(
            serde_json::from_str::<RemoteScrapeError>(&json).unwrap(),
            err
        );
    }
}
