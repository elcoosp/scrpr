//! Traits describing scrapable remote resources and element helpers.

use crate::error::RemoteScrapeError;

/// Describes how to scrape one kind of remote resource.
///
/// Implementors map a resource id to a URL, provide the CSS selector that
/// locates the interesting element(s) inside the page, and convert such an
/// element into their own [`Self::Output`] type.
///
/// # Example
///
/// ```
/// use scrpr::{ElementExt, RemoteScrapable, RemoteScrapeError};
///
/// struct AppIcon;
///
/// impl RemoteScrapable for AppIcon {
///     type Output = String;
///
///     fn id_url(&self, id: &str) -> String {
///         format!("https://store.example.com/app/{id}")
///     }
///
///     fn elem_into(
///         &self,
///         elem: scraper::ElementRef<'_>,
///     ) -> Result<Self::Output, RemoteScrapeError> {
///         elem.required_attr("src").map(ToOwned::to_owned)
///     }
///
///     fn res_selector(&self) -> &str {
///         "img.icon"
///     }
/// }
/// ```
pub trait RemoteScrapable: Send + Sync {
    /// Owned value produced from one matched element.
    type Output;

    /// The url formatted for a given id.
    fn id_url(&self, id: &str) -> String;

    /// Transform the element into [`Self::Output`].
    ///
    /// # Errors
    ///
    /// Returns [`RemoteScrapeError`] when the element does not carry the
    /// expected data (missing attribute, unparseable url...).
    fn elem_into(&self, elem: scraper::ElementRef<'_>) -> Result<Self::Output, RemoteScrapeError>;

    /// Resource selector.
    fn res_selector(&self) -> &str;
}

/// Convenience helpers implemented for [`scraper::ElementRef`].
///
/// They let [`RemoteScrapable::elem_into`] implementations extract data
/// without `unwrap`-ing on absent attributes.
pub trait ElementExt {
    /// Returns the attribute value, or [`RemoteScrapeError::MissingAttribute`]
    /// when the attribute is absent.
    ///
    /// # Errors
    ///
    /// Errors with [`RemoteScrapeError::MissingAttribute`] if the attribute
    /// does not exist on the element.
    fn required_attr(&self, name: &str) -> Result<&str, RemoteScrapeError>;

    /// Returns the concatenation of every text node inside the element.
    fn text_content(&self) -> String;
}

impl ElementExt for scraper::ElementRef<'_> {
    fn required_attr(&self, name: &str) -> Result<&str, RemoteScrapeError> {
        self.value()
            .attr(name)
            .ok_or_else(|| RemoteScrapeError::MissingAttribute(name.to_owned()))
    }

    fn text_content(&self) -> String {
        self.text().collect::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RemoteScrapeError;

    fn html_fragment() -> scraper::Html {
        scraper::Html::parse_fragment(
            r#"<div id="a" data-k="v">hello <b>world</b></div><div id="b"></div>"#,
        )
    }

    #[test]
    fn required_attr_returns_value() {
        let doc = html_fragment();
        let elem = doc
            .select(&scraper::Selector::parse("div#a").unwrap())
            .next()
            .unwrap();
        assert_eq!(elem.required_attr("data-k"), Ok("v"));
    }

    #[test]
    fn required_attr_errors_when_absent() {
        let doc = html_fragment();
        let elem = doc
            .select(&scraper::Selector::parse("div#a").unwrap())
            .next()
            .unwrap();
        assert_eq!(
            elem.required_attr("nope"),
            Err(RemoteScrapeError::MissingAttribute("nope".to_owned()))
        );
    }

    #[test]
    fn text_content_joins_all_text_nodes() {
        let doc = html_fragment();
        let elem = doc
            .select(&scraper::Selector::parse("div#a").unwrap())
            .next()
            .unwrap();
        assert_eq!(elem.text_content(), "hello world");
    }

    #[test]
    fn text_content_on_empty_element_is_empty() {
        let doc = html_fragment();
        let elem = doc
            .select(&scraper::Selector::parse("div#b").unwrap())
            .next()
            .unwrap();
        assert_eq!(elem.text_content(), "");
    }
}
