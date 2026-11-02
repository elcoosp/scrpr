//! Error types for the `scrpr` crate.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Every failure that can occur while fetching or extracting a document.
///
/// The type is [`Clone`], [`PartialEq`], [`Eq`], [`Serialize`] and
/// [`Deserialize`] so that scrape results can be stored, compared and sent
/// across threads or the network without losing the failure reason.
///
/// Every variant carries the context needed to debug a large batch: the url
/// for network-side failures, the selector for extraction-side failures.
///
/// # Example
///
/// ```
/// use scrpr::ScrapeError;
///
/// let err = ScrapeError::Http {
///     status: 503,
///     url: "https://example.net".to_owned(),
/// };
/// assert_eq!(
///     err.to_string(),
///     "server answered http 503 for https://example.net"
/// );
/// ```
#[derive(Debug, Error, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScrapeError {
    /// A network-level failure (DNS, connection reset, mid-body read...).
    #[error("network error while requesting {url}: {message}")]
    Network {
        /// Requested url.
        url: String,
        /// Underlying error message.
        message: String,
    },
    /// The request exceeded the configured timeout.
    #[error("request to {url} timed out")]
    Timeout {
        /// Requested url.
        url: String,
    },
    /// The server answered with a non-success, non-404 status code.
    #[error("server answered http {status} for {url}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Requested url.
        url: String,
    },
    /// The remote page answered with HTTP 404 Not Found.
    #[error("page not found: {url}")]
    PageNotFound {
        /// Requested url.
        url: String,
    },
    /// A url (page url, base url, or resolved link) could not be parsed.
    #[error("invalid url `{input}`: {message}")]
    UrlParse {
        /// The offending url text.
        input: String,
        /// Parse failure reason.
        message: String,
    },
    /// A CSS selector is not valid CSS.
    #[error("invalid selector `{selector}`: {message}")]
    Selector {
        /// The offending selector text.
        selector: String,
        /// Parse failure reason.
        message: String,
    },
    /// No element matched the selector.
    #[error("no element matched `{selector}`")]
    NoMatch {
        /// The selector that matched nothing.
        selector: String,
    },
    /// A required HTML attribute was absent from the matched element.
    #[error("missing attribute `{attribute}`{}", selector.as_ref().map_or(String::new(), |s| format!(" on element matching `{s}`")))]
    Attribute {
        /// The absent attribute name.
        attribute: String,
        /// The selector that located the element, when known.
        selector: Option<String>,
    },
    /// A extracted value could not be parsed into the requested type.
    #[error("failed to parse `{value}` as {target}")]
    Parse {
        /// The raw textual value.
        value: String,
        /// Target type name (from [`std::any::type_name`]).
        target: String,
    },
    /// The response body exceeded the configured size limit.
    #[error("response body from {url} exceeded the {limit} byte limit")]
    BodyTooLarge {
        /// Requested url.
        url: String,
        /// Configured limit in bytes.
        limit: usize,
    },
    /// The underlying HTTP client could not be built.
    #[error("scraper client configuration error: {0}")]
    Client(String),
    /// A JSON document could not be parsed.
    #[error("json error: {0}")]
    Json(String),
}

impl ScrapeError {
    pub(crate) fn url_parse(input: impl Into<String>, message: impl Into<String>) -> Self {
        Self::UrlParse {
            input: input.into(),
            message: message.into(),
        }
    }

    pub(crate) fn selector(selector: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Selector {
            selector: selector.into(),
            message: message.into(),
        }
    }
}

impl From<url::ParseError> for ScrapeError {
    fn from(err: url::ParseError) -> Self {
        Self::url_parse("", err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_explicit() {
        assert_eq!(
            ScrapeError::Network {
                url: "https://a".to_owned(),
                message: "boom".to_owned()
            }
            .to_string(),
            "network error while requesting https://a: boom"
        );
        assert_eq!(
            ScrapeError::Timeout {
                url: "https://a".to_owned()
            }
            .to_string(),
            "request to https://a timed out"
        );
        assert_eq!(
            ScrapeError::Http {
                status: 403,
                url: "https://a".to_owned()
            }
            .to_string(),
            "server answered http 403 for https://a"
        );
        assert_eq!(
            ScrapeError::PageNotFound {
                url: "https://a".to_owned()
            }
            .to_string(),
            "page not found: https://a"
        );
        assert_eq!(
            ScrapeError::url_parse("::nope::", "relative URL without a base").to_string(),
            "invalid url `::nope::`: relative URL without a base"
        );
        assert_eq!(
            ScrapeError::selector("<<<", "unexpected token").to_string(),
            "invalid selector `<<<`: unexpected token"
        );
        assert_eq!(
            ScrapeError::NoMatch {
                selector: "h1".to_owned()
            }
            .to_string(),
            "no element matched `h1`"
        );
        assert_eq!(
            ScrapeError::Attribute {
                attribute: "src".to_owned(),
                selector: None
            }
            .to_string(),
            "missing attribute `src`"
        );
        assert_eq!(
            ScrapeError::Attribute {
                attribute: "src".to_owned(),
                selector: Some("img.icon".to_owned())
            }
            .to_string(),
            "missing attribute `src` on element matching `img.icon`"
        );
        assert_eq!(
            ScrapeError::Parse {
                value: "x".to_owned(),
                target: "u32".to_owned()
            }
            .to_string(),
            "failed to parse `x` as u32"
        );
        assert_eq!(
            ScrapeError::BodyTooLarge {
                url: "https://a".to_owned(),
                limit: 1024
            }
            .to_string(),
            "response body from https://a exceeded the 1024 byte limit"
        );
        assert_eq!(
            ScrapeError::Client("bad header".to_owned()).to_string(),
            "scraper client configuration error: bad header"
        );
        assert_eq!(
            ScrapeError::Json("unexpected eof".to_owned()).to_string(),
            "json error: unexpected eof"
        );
    }

    #[test]
    fn url_parse_error_converts() {
        let err: ScrapeError = url::Url::parse("::nope::").unwrap_err().into();
        assert_eq!(
            err,
            ScrapeError::url_parse("", "relative URL without a base")
        );
    }

    #[test]
    fn error_serializes_and_deserializes() {
        let err = ScrapeError::PageNotFound {
            url: "https://example.net".to_owned(),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"PageNotFound":{"url":"https://example.net"}}"#);
        assert_eq!(serde_json::from_str::<ScrapeError>(&json).unwrap(), err);
    }

    #[test]
    fn every_variant_round_trips_through_json() {
        let errors = vec![
            ScrapeError::Network {
                url: "u".to_owned(),
                message: "m".to_owned(),
            },
            ScrapeError::Timeout {
                url: "u".to_owned(),
            },
            ScrapeError::Http {
                status: 500,
                url: "u".to_owned(),
            },
            ScrapeError::PageNotFound {
                url: "u".to_owned(),
            },
            ScrapeError::url_parse("in", "msg"),
            ScrapeError::selector("sel", "msg"),
            ScrapeError::NoMatch {
                selector: "sel".to_owned(),
            },
            ScrapeError::Attribute {
                attribute: "a".to_owned(),
                selector: Some("s".to_owned()),
            },
            ScrapeError::Attribute {
                attribute: "a".to_owned(),
                selector: None,
            },
            ScrapeError::Parse {
                value: "v".to_owned(),
                target: "t".to_owned(),
            },
            ScrapeError::BodyTooLarge {
                url: "u".to_owned(),
                limit: 1,
            },
            ScrapeError::Client("c".to_owned()),
            ScrapeError::Json("j".to_owned()),
        ];
        for err in errors {
            let json = serde_json::to_string(&err).unwrap();
            assert_eq!(serde_json::from_str::<ScrapeError>(&json).unwrap(), err);
        }
    }
}
