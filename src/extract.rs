//! Typed extraction: the [`Extract`] trait, the [`Extractor`] combinator
//! DSL and the [`field`] constructors.
//!
//! Two complementary styles:
//!
//! 1. **Structs**: implement [`Extract`] for your own type (serde-like) and
//!    scrape with [`crate::scrape_html::<E>`] or
//!    [`crate::Scraper::fetch_as`].
//! 2. **Ad-hoc fields**: build [`Extractor`] values with the [`field`]
//!    constructors, transform them (`map`, `parse`, `optional`...), and run
//!    them directly or together through the tuple [`FieldSet`] impls.

use std::any::type_name;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::error::ScrapeError;
use crate::page::Page;

/// A type that can build itself from a [`Page`].
///
/// This is the serde-style entry point: implement it once for your domain
/// type, then use it with [`crate::scrape_html`], [`crate::Scraper::fetch_as`],
/// [`crate::Scraper::scrape`] or [`crate::Scraper::scrape_stream`].
///
/// # Example
///
/// ```
/// use scrpr::{Extract, Page, ScrapeError, field};
///
/// struct Product {
///     title: String,
///     price: f64,
/// }
///
/// impl Extract for Product {
///     fn extract(page: &Page) -> Result<Self, ScrapeError> {
///         let (title, price) = scrpr::extract(
///             page,
///             (
///                 field::text("h1.name"),
///                 field::text("span.price").parse::<f64>(),
///             ),
///         )?;
///         Ok(Self { title, price })
///     }
/// }
///
/// let page = Page::parse(r#"<h1 class="name">Chair</h1><span class="price">49.90</span>"#);
/// let product = Product::extract(&page).unwrap();
/// assert_eq!(product.title, "Chair");
/// assert!((product.price - 49.9).abs() < 1e-9);
/// ```
pub trait Extract: Send + Sized + 'static {
    /// Builds `Self` from the page.
    ///
    /// # Errors
    ///
    /// Returns any [`ScrapeError`] raised by the underlying selectors,
    /// attributes or value parsing.
    fn extract(page: &Page) -> Result<Self, ScrapeError>;
}

/// Shared, thread-safe extraction closure behind every [`Extractor`].
type ExtractFn<T> = Arc<dyn for<'a> Fn(&'a Page) -> Result<T, ScrapeError> + Send + Sync>;

/// A reusable, composable extraction producing a `T` from a [`Page`].
///
/// `Extractor` values are cheap to build, [`Clone`] (shared closure), and
/// transformable. Construct them with the [`field`] functions and run them
/// with [`Extractor::get`].
///
/// # Example
///
/// ```
/// use scrpr::{field, Page};
///
/// let price = field::text("span.price").parse::<f64>();
///
/// let page = Page::parse(r#"<span class="price">19.5</span>"#);
/// assert!((price.get(&page).unwrap() - 19.5).abs() < 1e-9);
/// ```
pub struct Extractor<T: 'static> {
    label: String,
    func: ExtractFn<T>,
}

impl<T: 'static> Extractor<T> {
    pub(crate) fn new(
        label: impl Into<String>,
        func: impl for<'a> Fn(&'a Page) -> Result<T, ScrapeError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            func: Arc::new(func),
        }
    }

    /// Human-readable description of the extraction, e.g. `text("h1")`.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Runs the extraction against `page`.
    ///
    /// # Errors
    ///
    /// Returns the [`ScrapeError`] produced by the underlying extraction.
    pub fn get(&self, page: &Page) -> Result<T, ScrapeError> {
        (self.func)(page)
    }

    /// Transforms the output value.
    #[must_use]
    pub fn map<U>(self, f: impl Fn(T) -> U + Send + Sync + 'static) -> Extractor<U> {
        let Self { label, func } = self;
        Extractor::new(label, move |page| func(page).map(&f))
    }

    /// Transforms the output value, potentially failing.
    #[must_use]
    pub fn try_map<U>(
        self,
        f: impl Fn(T) -> Result<U, ScrapeError> + Send + Sync + 'static,
    ) -> Extractor<U> {
        let Self { label, func } = self;
        Extractor::new(label, move |page| func(page).and_then(&f))
    }

    /// Parses the extracted text into `F` (anything implementing
    /// [`FromStr`]).
    #[must_use]
    pub fn parse<F>(self) -> Extractor<F>
    where
        T: fmt::Display,
        F: FromStr + 'static,
    {
        let Self { label, func } = self;
        let target = type_name::<F>();
        Extractor::new(label, move |page| {
            let raw = func(page)?;
            raw.to_string()
                .parse::<F>()
                .map_err(|_| ScrapeError::Parse {
                    value: raw.to_string(),
                    target: target.to_owned(),
                })
        })
    }

    /// Maps a missing element to `None`. Other failures (invalid selector,
    /// missing attribute, parse errors) still propagate.
    #[must_use]
    pub fn optional(self) -> Extractor<Option<T>> {
        let Self { label, func } = self;
        Extractor::new(label, move |page| match func(page) {
            Ok(value) => Ok(Some(value)),
            Err(ScrapeError::NoMatch { .. }) => Ok(None),
            Err(err) => Err(err),
        })
    }

    /// Falls back to `fallback` when the element is missing. Other failures
    /// still propagate.
    #[must_use]
    pub fn or(self, fallback: T) -> Extractor<T>
    where
        T: Clone + Send + Sync,
    {
        let Self { label, func } = self;
        Extractor::new(label, move |page| match func(page) {
            Ok(value) => Ok(value),
            Err(ScrapeError::NoMatch { .. }) => Ok(fallback.clone()),
            Err(err) => Err(err),
        })
    }

    /// Falls back to [`Default::default`] when the element is missing.
    #[must_use]
    pub fn or_default(self) -> Extractor<T>
    where
        T: Default + Clone + Send + Sync,
    {
        self.or(T::default())
    }
}

impl<T: 'static> Clone for Extractor<T> {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            func: Arc::clone(&self.func),
        }
    }
}

impl<T: 'static> fmt::Debug for Extractor<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Extractor").field(&self.label).finish()
    }
}

/// Runs several [`Extractor`]s against one page and collects their outputs.
///
/// Implemented for tuples of up to twelve `Extractor<_>` values (owned or
/// borrowed); this is the cheapest way to pull many fields from one parsed
/// page.
///
/// # Errors
///
/// Returns the first [`ScrapeError`] produced by any field.
///
/// # Example
///
/// ```
/// use scrpr::{extract, field, Page};
///
/// let title = field::text("h1");
/// let link = field::attr("a.more", "href");
///
/// let page = Page::parse(r#"<h1>Hello</h1><a class="more" href="/more">more</a>"#);
/// let (title, link) = extract(&page, (&title, &link)).unwrap();
/// assert_eq!(title, "Hello");
/// assert_eq!(link, "/more");
///
/// // Owned extractors work too.
/// let (title, link) = extract(&page, (field::text("h1"), field::attr("a.more", "href"))).unwrap();
/// assert_eq!(title, "Hello");
/// ```
#[must_use = "extraction without reading the result has no effect"]
#[allow(clippy::needless_pass_by_value)] // accepts owned and borrowed tuples uniformly
pub fn extract<F: FieldSet>(page: &Page, fields: F) -> Result<F::Output, ScrapeError> {
    fields.get_all(page)
}

/// Tuple of [`Extractor`] references, extractable in one pass.
pub trait FieldSet {
    /// Tuple of every field output, in order.
    type Output;

    /// Runs every field against `page`.
    ///
    /// # Errors
    ///
    /// Returns the first [`ScrapeError`] produced by any field.
    fn get_all(&self, page: &Page) -> Result<Self::Output, ScrapeError>;
}

macro_rules! impl_field_set {
    ($($name:ident),+) => {
        impl<$($name: 'static),+> FieldSet for ($( &Extractor<$name>, )+) {
            type Output = ($($name,)+);

            #[allow(clippy::many_single_char_names)]
            fn get_all(&self, page: &Page) -> Result<Self::Output, ScrapeError> {
                #[allow(non_snake_case)]
                let ($(ref $name,)+) = *self;
                Ok(($($name.get(page)?,)+))
            }
        }

        impl<$($name: 'static),+> FieldSet for ($( Extractor<$name>, )+) {
            type Output = ($($name,)+);

            #[allow(clippy::many_single_char_names)]
            fn get_all(&self, page: &Page) -> Result<Self::Output, ScrapeError> {
                #[allow(non_snake_case)]
                let ($(ref $name,)+) = *self;
                Ok(($($name.get(page)?,)+))
            }
        }
    };
}

impl_field_set!(A);
impl_field_set!(A, B);
impl_field_set!(A, B, C);
impl_field_set!(A, B, C, D);
impl_field_set!(A, B, C, D, E);
impl_field_set!(A, B, C, D, E, F);
impl_field_set!(A, B, C, D, E, F, G);
impl_field_set!(A, B, C, D, E, F, G, H);
impl_field_set!(A, B, C, D, E, F, G, H, I);
impl_field_set!(A, B, C, D, E, F, G, H, I, J);
impl_field_set!(A, B, C, D, E, F, G, H, I, J, K);
impl_field_set!(A, B, C, D, E, F, G, H, I, J, K, L);

/// Runs a single [`Extract`] type against an already-parsed page.
///
/// Equivalent to `E::extract(page)`; handy in generic pipelines.
///
/// # Errors
///
/// Returns the [`ScrapeError`] produced by `E::extract`.
pub fn extract_from<E: Extract>(page: &Page) -> Result<E, ScrapeError> {
    E::extract(page)
}

/// Parses an HTML document and extracts `E` from it.
///
/// # Errors
///
/// Errors with [`ScrapeError::Selector`] when an extraction selector is
/// invalid and [`ScrapeError::NoMatch`] when nothing matches.
///
/// # Example
///
/// ```
/// use scrpr::{Extract, Page, ScrapeError, field};
///
/// struct Title(String);
///
/// impl Extract for Title {
///     fn extract(page: &Page) -> Result<Self, ScrapeError> {
///         Ok(Self(field::text("h1").get(page)?))
///     }
/// }
///
/// let html = "<html><body><h1>Hello</h1></body></html>";
/// assert_eq!(scrpr::scrape_html::<Title>(html).unwrap().0, "Hello");
/// ```
pub fn scrape_html<E: Extract>(html: &str) -> Result<E, ScrapeError> {
    E::extract(&Page::parse(html))
}

/// Parses an HTML document and runs one [`Extractor`] against it.
///
/// # Errors
///
/// Errors with [`ScrapeError::Selector`] when the selector is invalid and
/// [`ScrapeError::NoMatch`] when nothing matches.
///
/// # Example
///
/// ```
/// use scrpr::{field, scrape_html_with};
///
/// let link = field::attr("a.more", "href");
/// let html = r#"<a class="more" href="/more">more</a>"#;
/// assert_eq!(scrape_html_with(html, &link).unwrap(), "/more");
/// ```
pub fn scrape_html_with<T>(html: &str, extractor: &Extractor<T>) -> Result<T, ScrapeError> {
    extractor.get(&Page::parse(html))
}

/// Constructors for common [`Extractor`]s.
///
/// Every constructor is *required* by default: a missing element yields
/// [`ScrapeError::NoMatch`]. Chain [`Extractor::optional`] or
/// [`Extractor::or`] to tolerate absence.
///
/// # Example
///
/// ```
/// use scrpr::{field, Page};
///
/// let page = Page::parse(r#"<img class="icon" src="/i.png"><p>hi</p>"#);
///
/// assert_eq!(field::attr("img.icon", "src").get(&page).unwrap(), "/i.png");
/// assert_eq!(field::text("p").get(&page).unwrap(), "hi");
/// ```
pub mod field {
    use super::Extractor;
    use crate::error::ScrapeError;
    use crate::page::ElementExt;

    /// Text of the first element matching `selector` (all text nodes joined).
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn text(selector: &str) -> Extractor<String> {
        let selector = selector.to_owned();
        Extractor::new(format!("text({selector:?})"), move |page| {
            Ok(page.select_required(&selector)?.text_content())
        })
    }

    /// Text of every element matching `selector`, in document order.
    ///
    /// Errors with [`ScrapeError::NoMatch`] when nothing matches; chain
    /// `optional()` to allow an empty result instead.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn text_all(selector: &str) -> Extractor<Vec<String>> {
        let selector = selector.to_owned();
        Extractor::new(format!("text_all({selector:?})"), move |page| {
            let values: Vec<String> = page
                .select(&selector)?
                .into_iter()
                .map(crate::page::text_content_of)
                .collect();
            if values.is_empty() {
                return Err(ScrapeError::NoMatch {
                    selector: selector.clone(),
                });
            }
            Ok(values)
        })
    }

    /// Attribute value of the first element matching `selector`.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn attr(selector: &str, attribute: &str) -> Extractor<String> {
        let selector = selector.to_owned();
        let attribute = attribute.to_owned();
        Extractor::new(format!("attr({selector:?}, {attribute:?})"), move |page| {
            let elem = page.select_required(&selector)?;
            elem.value()
                .attr(&attribute)
                .map(ToOwned::to_owned)
                .ok_or_else(|| ScrapeError::Attribute {
                    attribute: attribute.clone(),
                    selector: Some(selector.clone()),
                })
        })
    }

    /// Attribute value of the first element matching `selector`, or `None`
    /// when the element *or the attribute* is absent.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn attr_opt(selector: &str, attribute: &str) -> Extractor<Option<String>> {
        let selector = selector.to_owned();
        let attribute = attribute.to_owned();
        Extractor::new(
            format!("attr_opt({selector:?}, {attribute:?})"),
            move |page| {
                Ok(page
                    .select_first(&selector)?
                    .and_then(|elem| elem.value().attr(&attribute))
                    .map(ToOwned::to_owned))
            },
        )
    }

    /// Attribute value of every element matching `selector`.
    ///
    /// Errors with [`ScrapeError::Attribute`] if any matched element lacks
    /// the attribute.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn attr_all(selector: &str, attribute: &str) -> Extractor<Vec<String>> {
        let selector = selector.to_owned();
        let attribute = attribute.to_owned();
        Extractor::new(
            format!("attr_all({selector:?}, {attribute:?})"),
            move |page| {
                let elems = page.select(&selector)?;
                if elems.is_empty() {
                    return Err(ScrapeError::NoMatch {
                        selector: selector.clone(),
                    });
                }
                elems
                    .iter()
                    .map(|elem| {
                        elem.value()
                            .attr(&attribute)
                            .map(ToOwned::to_owned)
                            .ok_or_else(|| ScrapeError::Attribute {
                                attribute: attribute.clone(),
                                selector: Some(selector.clone()),
                            })
                    })
                    .collect()
            },
        )
    }

    /// Attribute value of the first element matching `selector`, resolved
    /// into an absolute url against the page url.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn attr_url(selector: &str, attribute: &str) -> Extractor<url::Url> {
        let selector = selector.to_owned();
        let attribute = attribute.to_owned();
        Extractor::new(
            format!("attr_url({selector:?}, {attribute:?})"),
            move |page| {
                let elem = page.select_required(&selector)?;
                let raw = elem
                    .value()
                    .attr(&attribute)
                    .ok_or_else(|| ScrapeError::Attribute {
                        attribute: attribute.clone(),
                        selector: Some(selector.clone()),
                    })?;
                page.resolve(raw)
            },
        )
    }

    /// Serialized children of the first element matching `selector`
    /// (without the enclosing tag).
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn inner_html(selector: &str) -> Extractor<String> {
        let selector = selector.to_owned();
        Extractor::new(format!("inner_html({selector:?})"), move |page| {
            Ok(page.select_required(&selector)?.inner_html())
        })
    }

    /// Serialized first element matching `selector`, tags included.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn outer_html(selector: &str) -> Extractor<String> {
        let selector = selector.to_owned();
        Extractor::new(format!("outer_html({selector:?})"), move |page| {
            Ok(page.select_required(&selector)?.outer_html())
        })
    }

    /// Every `<a href>` target, resolved against the page url.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn links() -> Extractor<Vec<url::Url>> {
        Extractor::new("links()", |page| Ok(page.links()))
    }

    /// The page url itself. Errors when the page was parsed without a base
    /// url.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn page_url() -> Extractor<url::Url> {
        Extractor::new("page_url()", |page| {
            page.url().cloned().ok_or_else(|| {
                ScrapeError::url_parse(
                    "<page>",
                    "page was parsed without a base url; use Page::parse_with_base",
                )
            })
        })
    }

    /// Text of the `<title>` element.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn title() -> Extractor<String> {
        Extractor::new("title()", |page| {
            page.title().ok_or_else(|| ScrapeError::NoMatch {
                selector: "title".to_owned(),
            })
        })
    }

    /// `content` of `<meta name="{name}">`.
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn meta(name: &str) -> Extractor<String> {
        let name = name.to_owned();
        Extractor::new(format!("meta({name:?})"), move |page| {
            page.meta(&name).ok_or_else(|| ScrapeError::NoMatch {
                selector: format!("meta[name={name:?}]"),
            })
        })
    }

    /// `content` of `<meta property="{property}">` (Open Graph).
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn meta_property(property: &str) -> Extractor<String> {
        let property = property.to_owned();
        Extractor::new(format!("meta_property({property:?})"), move |page| {
            page.meta_property(&property)
                .ok_or_else(|| ScrapeError::NoMatch {
                    selector: format!("meta[property={property:?}]"),
                })
        })
    }

    /// Every `<script type="application/ld+json">` payload, parsed.
    ///
    /// Returns an empty vec when the page carries no structured data; chain
    /// `try_map` to turn that into an error.
    #[cfg(feature = "json")]
    #[must_use = "build the extractor, then call `.get` or `scrape_html_with`"]
    pub fn json_ld() -> Extractor<Vec<serde_json::Value>> {
        Extractor::new("json_ld()", |page| Ok(page.json_ld()))
    }
}

#[cfg(feature = "json")]
#[cfg(test)]
mod json_tests {
    use super::*;

    #[test]
    fn json_ld_extractor_returns_every_payload() {
        let page = Page::parse(
            r#"<script type="application/ld+json">{"a":1}</script>
               <script type="application/ld+json">[2,3]</script>"#,
        );
        let values = field::json_ld().get(&page).unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["a"], 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_extracts_first_match() {
        let page = Page::parse(r#"<p class="a">one</p><p class="a">two</p>"#);
        assert_eq!(field::text("p.a").get(&page).unwrap(), "one");
    }

    #[test]
    fn text_errors_without_match() {
        let err = field::text("p.missing").get(&Page::parse("")).unwrap_err();
        assert_eq!(
            err,
            ScrapeError::NoMatch {
                selector: "p.missing".to_owned()
            }
        );
    }

    #[test]
    fn text_all_collects_every_match() {
        let page = Page::parse("<li>1</li><li>2</li><li>3</li>");
        assert_eq!(
            field::text_all("li").get(&page).unwrap(),
            vec!["1".to_owned(), "2".to_owned(), "3".to_owned()]
        );
        assert!(field::text_all("li").get(&Page::parse("")).is_err());
    }

    #[test]
    fn attr_variants() {
        let page = Page::parse(r#"<a class="m" href="/x">m</a>"#);
        assert_eq!(field::attr("a.m", "href").get(&page).unwrap(), "/x");
        assert_eq!(
            field::attr_opt("a.m", "href").get(&page).unwrap(),
            Some("/x".to_owned())
        );
        assert_eq!(field::attr_opt("a.m", "missing").get(&page).unwrap(), None);
        assert_eq!(
            field::attr_all("a", "href").get(&page).unwrap(),
            vec!["/x".to_owned()]
        );

        let missing = field::attr("a.m", "missing").get(&page).unwrap_err();
        assert_eq!(
            missing,
            ScrapeError::Attribute {
                attribute: "missing".to_owned(),
                selector: Some("a.m".to_owned())
            }
        );
    }

    #[test]
    fn attr_all_requires_every_element_to_carry_the_attribute() {
        let page = Page::parse(r#"<a href="/1">1</a><a>2</a>"#);
        let err = field::attr_all("a", "href").get(&page).unwrap_err();
        assert!(matches!(err, ScrapeError::Attribute { .. }));
    }

    #[test]
    fn attr_url_resolves_against_page_url() {
        let page =
            Page::parse_with_base(r#"<a href="items/1">x</a>"#, "https://s.test/dir/page").unwrap();
        assert_eq!(
            field::attr_url("a", "href").get(&page).unwrap(),
            url::Url::parse("https://s.test/dir/items/1").unwrap()
        );
        let err = field::attr_url("a", "href")
            .get(&Page::parse(r#"<a href="/x">x</a>"#))
            .unwrap_err();
        assert!(matches!(err, ScrapeError::UrlParse { .. }));
    }

    #[test]
    fn html_extractors() {
        let page = Page::parse(r#"<div class="w">a<b>b</b>c</div>"#);
        assert_eq!(field::inner_html("div.w").get(&page).unwrap(), "a<b>b</b>c");
        assert_eq!(
            field::outer_html("div.w").get(&page).unwrap(),
            r#"<div class="w">a<b>b</b>c</div>"#
        );
    }

    #[test]
    fn links_page_url_title_and_meta_extractors() {
        let page = Page::parse_with_base(
            r#"<head><title>T</title><meta name="d" content="dd"><meta property="og:x" content="xx"></head>
               <body><a href="/1">1</a></body>"#,
            "https://s.test",
        )
        .unwrap();
        assert_eq!(
            field::links().get(&page).unwrap(),
            vec![url::Url::parse("https://s.test/1").unwrap()]
        );
        assert_eq!(
            field::page_url().get(&page).unwrap(),
            url::Url::parse("https://s.test").unwrap()
        );
        assert_eq!(field::title().get(&page).unwrap(), "T");
        assert_eq!(field::meta("d").get(&page).unwrap(), "dd");
        assert_eq!(field::meta_property("og:x").get(&page).unwrap(), "xx");

        let err = field::page_url().get(&Page::parse("")).unwrap_err();
        assert!(matches!(err, ScrapeError::UrlParse { .. }));
        assert!(field::title().get(&Page::parse("")).is_err());
        assert!(field::meta("d").get(&Page::parse("")).is_err());
        assert!(field::meta_property("og:x").get(&Page::parse("")).is_err());
    }

    #[test]
    fn combinators_compose() {
        let page = Page::parse(r#"<span class="p">12.5</span>"#);
        let price = field::text("span.p");

        let doubled = price.clone().parse::<f64>().map(|p| p * 2.0);
        assert!((doubled.get(&page).unwrap() - 25.0).abs() < 1e-9);

        let checked = price.clone().parse::<f64>().try_map(|p| {
            if p < 0.0 {
                Err(ScrapeError::Json("negative".into()))
            } else {
                Ok(p)
            }
        });
        assert!((checked.get(&page).unwrap() - 12.5).abs() < 1e-9);

        let bad_parse = field::text("span.p").parse::<u32>().get(&page).unwrap_err();
        assert!(matches!(bad_parse, ScrapeError::Parse { .. }));

        let absent = Page::parse("");
        assert_eq!(field::text("span.p").optional().get(&absent).unwrap(), None);
        assert_eq!(
            field::text("span.p")
                .or("n/a".to_owned())
                .get(&absent)
                .unwrap(),
            "n/a"
        );
        assert_eq!(field::text("span.p").or_default().get(&absent).unwrap(), "");

        // Non-NoMatch failures still propagate through `optional`.
        let invalid_selector = field::text("<<<bad").optional();
        assert!(invalid_selector.get(&absent).is_err());
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn extract_tuples_and_labels() {
        let title = field::text("h1");
        let price = field::text(".p").parse::<f64>();
        assert_eq!(title.label(), "text(\"h1\")");
        assert_eq!(Extractor::label(&price), "text(\".p\")");
        assert_eq!(format!("{title:?}"), "Extractor(\"text(\\\"h1\\\")\")");

        let page = Page::parse(r#"<h1>Hi</h1><span class="p">3</span>"#);
        let (t, p): (String, f64) = extract(&page, (&title, &price)).unwrap();
        assert_eq!(t, "Hi");
        assert!((p - 3.0).abs() < 1e-9);

        // Arity 12 exercises the widest generated impl.
        let fields = (
            &title, &title, &title, &title, &title, &title, &title, &title, &title, &title, &title,
            &price,
        );
        let (a, b, c, d, e, f, g, h, i, j, k, p): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            f64,
        ) = extract(&page, fields).unwrap();
        assert_eq!((a.as_str(), p), ("Hi", 3.0));
        let _ = (b, c, d, e, f, g, h, i, j, k);
    }

    #[test]
    fn clone_shares_the_underlying_closure() {
        let extractor = field::text("h1");
        let cloned = extractor.clone();
        let page = Page::parse("<h1>same</h1>");
        assert_eq!(extractor.get(&page).unwrap(), cloned.get(&page).unwrap());
    }

    #[test]
    fn scrape_html_helpers() {
        struct Title(String);
        impl Extract for Title {
            fn extract(page: &Page) -> Result<Self, ScrapeError> {
                Ok(Self(field::text("h1").get(page)?))
            }
        }

        let html = "<html><body><h1>Hello</h1></body></html>";
        assert_eq!(scrape_html::<Title>(html).unwrap().0, "Hello");
        assert_eq!(
            extract_from::<Title>(&Page::parse(html)).unwrap().0,
            "Hello"
        );
        assert_eq!(scrape_html_with(html, &field::text("h1")).unwrap(), "Hello");
        assert!(scrape_html::<Title>("").is_err());
        assert!(scrape_html_with("", &field::text("h1")).is_err());
        assert!(matches!(
            scrape_html_with("", &field::text("<<<bad")).unwrap_err(),
            ScrapeError::Selector { .. }
        ));
    }
}
