//! The parsed document handle shared by every extraction API.

use crate::error::ScrapeError;
use crate::selector::cached_selector;

pub use scraper::ElementRef;

/// A parsed HTML document together with the url it was fetched from (when
/// it has one).
///
/// Every extraction in the crate operates on a `&Page`, which makes
/// extraction independent from how the document arrived: parsed from a
/// string, fetched over the network, or served from the cache.
///
/// # Example
///
/// ```
/// use scrpr::Page;
///
/// let page = Page::parse(r#"<html><head><title>Catalog</title></head>
///     <body><a href="/items/1">one</a></body></html>"#);
///
/// assert_eq!(page.title().as_deref(), Some("Catalog"));
/// assert!(page.select_first("a").is_ok());
/// ```
pub struct Page {
    url: Option<url::Url>,
    html: scraper::Html,
}

impl Page {
    /// Parses an HTML document without a base url.
    ///
    /// Relative links cannot be resolved; see [`Page::parse_with_base`] and
    /// [`Page::resolve`].
    #[must_use]
    pub fn parse(html: &str) -> Self {
        Self {
            url: None,
            html: scraper::Html::parse_document(html),
        }
    }

    /// Parses an HTML document and remembers its base url so relative links
    /// and attribute urls can be resolved into absolute ones.
    ///
    /// # Errors
    ///
    /// Errors with [`ScrapeError::UrlParse`] when `base` is not a valid url.
    pub fn parse_with_base(html: &str, base: &str) -> Result<Self, ScrapeError> {
        let url =
            url::Url::parse(base).map_err(|err| ScrapeError::url_parse(base, err.to_string()))?;
        Ok(Self {
            url: Some(url),
            html: scraper::Html::parse_document(html),
        })
    }

    #[must_use]
    pub(crate) fn with_url(html: &str, url: url::Url) -> Self {
        Self {
            url: Some(url),
            html: scraper::Html::parse_document(html),
        }
    }

    /// The url the page was fetched from, when known.
    #[must_use]
    pub fn url(&self) -> Option<&url::Url> {
        self.url.as_ref()
    }

    /// The underlying [`scraper::Html`] document, for full-power queries.
    #[must_use]
    pub fn document(&self) -> &scraper::Html {
        &self.html
    }

    /// Selects every element matching the CSS selector.
    ///
    /// # Errors
    ///
    /// Errors with [`ScrapeError::Selector`] when the selector is invalid.
    pub fn select(&self, selector: &str) -> Result<Vec<ElementRef<'_>>, ScrapeError> {
        let parsed = cached_selector(selector)?;
        Ok(self.html.select(parsed.as_ref()).collect())
    }

    /// Selects the first element matching the CSS selector, if any.
    ///
    /// # Errors
    ///
    /// Errors with [`ScrapeError::Selector`] when the selector is invalid.
    pub fn select_first(&self, selector: &str) -> Result<Option<ElementRef<'_>>, ScrapeError> {
        Ok(self.html.select(cached_selector(selector)?.as_ref()).next())
    }

    pub(crate) fn select_required(&self, selector: &str) -> Result<ElementRef<'_>, ScrapeError> {
        self.select_first(selector)?
            .ok_or_else(|| ScrapeError::NoMatch {
                selector: selector.to_owned(),
            })
    }

    /// Every `<a href>` target, resolved against the page url when it has
    /// one. Without a base url only already-absolute hrefs are returned.
    #[must_use]
    pub fn links(&self) -> Vec<url::Url> {
        let Ok(anchor) = cached_selector("a[href]") else {
            return Vec::new();
        };
        match &self.url {
            Some(base) => self
                .html
                .select(anchor.as_ref())
                .filter_map(|elem| elem.value().attr("href"))
                .filter_map(|href| base.join(href).ok())
                .collect(),
            None => self
                .html
                .select(anchor.as_ref())
                .filter_map(|elem| elem.value().attr("href"))
                .filter_map(|href| url::Url::parse(href).ok())
                .collect(),
        }
    }

    /// The text of the `<title>` element, if present.
    #[must_use]
    pub fn title(&self) -> Option<String> {
        self.select_first("title")
            .ok()
            .flatten()
            .map(crate::page::text_content_of)
    }

    /// The `content` of `<meta name="{name}">`, if present.
    #[must_use]
    pub fn meta(&self, name: &str) -> Option<String> {
        self.meta_by("name", name)
    }

    /// The `content` of `<meta property="{property}">` (Open Graph), if
    /// present.
    #[must_use]
    pub fn meta_property(&self, property: &str) -> Option<String> {
        self.meta_by("property", property)
    }

    /// The raw text of every `<script>` element, in document order.
    #[must_use]
    pub fn scripts(&self) -> Vec<String> {
        let Ok(script) = cached_selector("script") else {
            return Vec::new();
        };
        self.html
            .select(script.as_ref())
            .map(crate::page::text_content_of)
            .collect()
    }

    /// Every `<script type="application/ld+json">` payload, parsed.
    ///
    /// Structured-data entries are the fastest reliable source of product,
    /// article and organization metadata; invalid JSON payloads are skipped.
    ///
    /// Requires the (default) `json` feature.
    #[cfg(feature = "json")]
    #[must_use]
    pub fn json_ld(&self) -> Vec<serde_json::Value> {
        let Ok(script) = cached_selector("script[type=\"application/ld+json\"]") else {
            return Vec::new();
        };
        self.html
            .select(script.as_ref())
            .map(crate::page::text_content_of)
            .filter(|text| !text.trim().is_empty())
            .filter_map(|text| match serde_json::from_str(&text) {
                Ok(value) => Some(value),
                Err(err) => {
                    tracing::debug!(error = %err, "skipping invalid ld+json payload");
                    None
                }
            })
            .collect()
    }

    /// Resolves `href` (absolute or relative) against the page url.
    ///
    /// # Errors
    ///
    /// Errors with [`ScrapeError::UrlParse`] when the page has no base url
    /// or when `href` cannot be resolved against it.
    pub fn resolve(&self, href: &str) -> Result<url::Url, ScrapeError> {
        let base = self.url.as_ref().ok_or_else(|| {
            ScrapeError::url_parse(
                href,
                "page has no base url; use Page::parse_with_base or fetch the page remotely",
            )
        })?;
        base.join(href)
            .map_err(|err| ScrapeError::url_parse(href, err.to_string()))
    }

    fn meta_by(&self, kind: &str, key: &str) -> Option<String> {
        let Ok(meta) = cached_selector("meta") else {
            return None;
        };
        self.html
            .select(meta.as_ref())
            .find(|elem| elem.value().attr(kind) == Some(key))
            .and_then(|elem| elem.value().attr("content"))
            .map(ToOwned::to_owned)
    }
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

pub(crate) fn text_content_of(element: ElementRef<'_>) -> String {
    element.text().collect::<String>()
}

/// Convenience helpers implemented for [`ElementRef`].
///
/// They let [`crate::Extract`] implementations pull data out of elements
/// without `unwrap`-ing on absent attributes.
pub trait ElementExt {
    /// Returns the attribute value, or [`ScrapeError::Attribute`] when the
    /// attribute is absent.
    ///
    /// # Errors
    ///
    /// Errors with [`ScrapeError::Attribute`] if the attribute does not
    /// exist on the element.
    fn required_attr(&self, name: &str) -> Result<&str, ScrapeError>;

    /// Returns the concatenation of every text node inside the element.
    fn text_content(&self) -> String;

    /// Serializes the element's children (without the enclosing tag).
    fn inner_html(&self) -> String;

    /// Serializes the element including its opening and closing tags.
    fn outer_html(&self) -> String;
}

impl ElementExt for ElementRef<'_> {
    fn required_attr(&self, name: &str) -> Result<&str, ScrapeError> {
        self.value()
            .attr(name)
            .ok_or_else(|| ScrapeError::Attribute {
                attribute: name.to_owned(),
                selector: None,
            })
    }

    fn text_content(&self) -> String {
        text_content_of(*self)
    }

    fn inner_html(&self) -> String {
        ElementRef::inner_html(self)
    }

    fn outer_html(&self) -> String {
        ElementRef::html(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_page() -> Page {
        Page::parse_with_base(
            r##"<html><head><title>Catalog</title>
                <meta name="description" content="All items">
                <meta name="no-content" value="x">
                <meta property="og:image" content="https://img.test/og.png">
                <script>var a = 1;</script>
                <script type="application/ld+json">{"@type":"Product","name":"Chair"}</script>
                <script type="application/ld+json">not json</script>
                </head><body>
                <a href="/items/1">one</a>
                <a href="https://elsewhere.test/x">two</a>
                <a href="::::bad">three</a>
                <a>no href</a>
                <a href="#frag">four</a>
                </body></html>"##,
            "https://shop.test/catalog",
        )
        .unwrap()
    }

    #[test]
    fn parse_without_base_has_no_url() {
        let page = Page::parse("<html></html>");
        assert!(page.url().is_none());
        assert!(page.resolve("/x").is_err());
    }

    #[test]
    fn parse_with_base_rejects_invalid_base() {
        let err = Page::parse_with_base("<html></html>", "::no::").unwrap_err();
        assert!(matches!(err, ScrapeError::UrlParse { .. }));
    }

    #[test]
    fn select_returns_all_matches_and_errors_on_invalid_selector() {
        let page = catalog_page();
        assert_eq!(page.select("a").unwrap().len(), 5);
        assert!(matches!(
            page.select("<<<bad").unwrap_err(),
            ScrapeError::Selector { .. }
        ));
    }

    #[test]
    fn select_first_and_select_required() {
        let page = catalog_page();
        assert!(page.select_first("a").unwrap().is_some());
        assert!(page.select_first("video").unwrap().is_none());
        assert!(page.select_required("a").is_ok());
        assert_eq!(
            page.select_required("video").unwrap_err(),
            ScrapeError::NoMatch {
                selector: "video".to_owned()
            }
        );
    }

    #[test]
    fn links_are_resolved_against_the_base_url() {
        let page = catalog_page();
        let links = page.links();
        assert_eq!(
            links,
            vec![
                url::Url::parse("https://shop.test/items/1").unwrap(),
                url::Url::parse("https://elsewhere.test/x").unwrap(),
                // url::join is lenient: junk hrefs resolve as relative paths.
                url::Url::parse("https://shop.test/::::bad").unwrap(),
                url::Url::parse("https://shop.test/catalog#frag").unwrap(),
            ]
        );
    }

    #[test]
    fn links_without_base_keep_absolute_targets_only() {
        let page = Page::parse(r#"<a href="/x">rel</a><a href="https://ok.test/">abs</a>"#);
        assert_eq!(
            page.links(),
            vec![url::Url::parse("https://ok.test/").unwrap()]
        );
    }

    #[test]
    fn title_meta_and_meta_property() {
        let page = catalog_page();
        assert_eq!(page.title().as_deref(), Some("Catalog"));
        assert_eq!(page.meta("description").as_deref(), Some("All items"));
        assert_eq!(page.meta("missing"), None);
        assert_eq!(
            page.meta_property("og:image").as_deref(),
            Some("https://img.test/og.png")
        );
        assert_eq!(page.meta_property("og:missing"), None);
    }

    #[test]
    fn scripts_collect_raw_text() {
        let page = catalog_page();
        assert_eq!(page.scripts().len(), 3);
        assert_eq!(page.scripts()[0], "var a = 1;");
    }

    #[cfg(feature = "json")]
    #[test]
    fn json_ld_parses_valid_payloads_and_skips_invalid_ones() {
        let page = catalog_page();
        let values = page.json_ld();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["@type"], "Product");
        assert_eq!(values[0]["name"], "Chair");
    }

    #[test]
    fn resolve_joins_relative_and_keeps_absolute() {
        let page = catalog_page();
        assert_eq!(
            page.resolve("/items/2").unwrap(),
            url::Url::parse("https://shop.test/items/2").unwrap()
        );
        assert_eq!(
            page.resolve("items/3").unwrap(),
            // RFC 3986: relative to the base's directory, and `/catalog`'s
            // directory is `/`.
            url::Url::parse("https://shop.test/items/3").unwrap()
        );
        assert_eq!(
            page.resolve("https://other.test/y").unwrap(),
            url::Url::parse("https://other.test/y").unwrap()
        );
    }

    #[test]
    fn debug_does_not_dump_the_whole_document() {
        let debug = format!("{:?}", catalog_page());
        assert!(debug.starts_with("Page { url:"));
        assert!(!debug.contains("Catalog"));
    }

    #[test]
    fn element_ext_helpers() {
        let page = Page::parse(r#"<div id="a" data-k="v">hello <b>world</b></div>"#);
        let elem = page.select_required("#a").unwrap();
        assert_eq!(elem.required_attr("data-k"), Ok("v"));
        assert!(elem.required_attr("missing").is_err());
        assert_eq!(elem.text_content(), "hello world");
        assert_eq!(elem.inner_html(), "hello <b>world</b>");
        // html5ever normalizes the attribute order when serializing.
        assert_eq!(
            elem.outer_html(),
            r#"<div data-k="v" id="a">hello <b>world</b></div>"#
        );
    }

    #[test]
    fn empty_page_has_no_title_meta_or_scripts() {
        let page = Page::parse("<html><body></body></html>");
        assert_eq!(page.title(), None);
        assert_eq!(page.meta("description"), None);
        assert_eq!(page.scripts(), [] as [String; 0]);
    }
}
