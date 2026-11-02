# Brainstorm & design rationale (v0.2.0 rewrite)

This document records the brainstorm that led to the v0.2.0 rewrite and the
decisions taken while implementing it.

## Starting point (v0.1.1)

A single `lib.rs` (~110 lines) with:

- a `RemoteScrapable` trait (`id_url`, `elem_into`, `res_selector`),
- a `remote_scrape` async function fanning out HTTP requests,
- one network-dependent test against Google Play.

### Issues found (review + `cargo clippy -- -W clippy::pedantic`)

| # | Issue | Consequence |
|---|-------|-------------|
| 1 | `reqwest::Client` built (and `.unwrap()`-ed) on every call | no connection pooling, panic on failure |
| 2 | `Selector::parse(...).unwrap()` re-parsed for **every id** | wasted CPU + panic on invalid selector |
| 3 | `rayon::par_iter` inside an async fn | rayon threads only *build* futures, they never poll them: pure overhead, zero parallelism benefit |
| 4 | Unbounded fan-out (`join_all` over all ids) | thousands of sockets/memory for large id batches |
| 5 | Only HTTP 404 handled; 403/429/5xx silently parsed as HTML | misleading `NoElementMatch` errors |
| 6 | No timeout / user agent / retries | poor scraping citizenship, hangs by default |
| 7 | `&Vec<String>` parameter | unidiomatic (clippy `ptr_arg`), forces allocation |
| 8 | `elem.attr("srcset").unwrap()` pattern | panic-prone API encouraged for implementors |
| 9 | Single network test against a live website | flaky, untestable CI |
| 10 | `derive_more::Display` alongside `thiserror` | duplicated error Display machinery |
| 11 | Missing docs everywhere | 15 pedantic warnings |

## Brainstorm

**Performance**

1. Replace rayon + `join_all` with `futures::StreamExt::buffered` for *bounded,
   configurable* concurrency.
2. One reused `reqwest::Client` per `RemoteScraper` (connection pooling), cheap
   to clone.
3. Parse the CSS selector once per batch, not per id.
4. Drop `rayon` and `derive_more` entirely.
5. Accept `&str` ids and `IntoIterator` inputs to avoid allocation churn.

**Modularity**

1. Split the monolith into `error.rs`, `traits.rs`, `scraper.rs`.
2. Builder-pattern `RemoteScraperBuilder` (timeout, user agent, concurrency,
   retries, retry delay).
3. Separate fetch from parsing so local HTML can be scraped offline.

**DRY**

1. One shared fetch+parse pipeline behind all entry points.
2. `thiserror` alone for `Display`/`Error` (drop `derive_more`).
3. `parse_selector` + `selector_errors` helpers instead of repeated
   `map_err`/`unwrap` boilerplate.

**KISS**

1. Keep the 3-method trait and its method names (backwards-compatible trait).
2. `ids: impl IntoIterator<Item: AsRef<str>>` accepts `&Vec<String>`,
   `Vec<&str>`, arrays, ...
3. No `unwrap`/`panic` in the library; every failure is a typed error.
4. Two dependencies fewer (`rayon`, `derive_more`).

**Complementary features (ambitious)**

1. Offline `scrape_html` (first match) and `scrape_html_all` (every match).
2. Retries with exponential backoff for network errors and HTTP 5xx.
3. Per-request timeout and configurable `User-Agent`.
4. New error variants: `HttpStatus(u16)`, `SelectorParse(String)`,
   `MissingAttribute(String)` + `From<url::ParseError>`.
5. `ElementExt` helper trait (`required_attr`, `text_content`).
6. Runnable, offline doctests for every public item.

## Implementation decisions

- **Dyn-erased internals**: the public API stays generic over
  `T: RemoteScrapable`, but the pipeline (`scrape_remote_dyn`,
  `scrape_html_dyn`, `scrape_first_erased`, `scrape_all_erased`) works on
  `&dyn RemoteScrapable<Output = O>`: one instantiation per output type
  instead of one per scrapable type. This shrinks the compiled code and keeps
  every branch testable.
- **Linear generic wrappers**: public generic functions contain no branching,
  so no instantiation can hide an untested path; all branching lives in the
  per-output-type erased functions.
- **Testing without dependencies**: a hand-rolled `std::net::TcpListener`
  HTTP server (`src/tests/server.rs`) serves canned responses (404, 403, 5xx,
  truncated bodies, slow responses, concurrency tracking) - no test-server
  crate needed, no network access in CI.
- **Coverage**: `cargo-llvm-cov` reports 100% regions, functions and lines.

## CI/CD

- `ci.yml`: rustfmt, clippy pedantic (`-D warnings`, lints also enforced in
  `Cargo.toml [lints]`), stable tests, MSRV 1.88 check, llvm-cov coverage
  uploaded to Codecov.
- `release.yml` (tag `v*`): tests, git-cliff release notes, refresh
  `CHANGELOG.md`, `cargo publish --locked`, GitHub release.
- `changelog.yml`: keeps `CHANGELOG.md` fresh on `main` with git-cliff.

---

# Brainstorm & design rationale (v0.3.0 breaking rewrite)

This section records the brainstorm that led to the v0.3.0 redesign. Unlike
v0.2.0 (which kept the original trait for compatibility), v0.3.0 is a
**deliberate, no-backward-compat redesign**: the goal is the best possible
declarative-scraping DX in Rust, not source compatibility.

## Starting point (v0.2.0)

v0.2.0 already fixed the v0.1.1 performance crimes (per-call clients, rayon in
async, unbounded fan-out, per-id selector parsing). What remained was an API
shape problem, not an implementation problem.

### Pain points of the v0.2.0 API

| # | Pain point | Consequence |
|---|------------|-------------|
| 1 | `RemoteScrapable` conflates three concerns (URL templating, selection, conversion) into one trait | One trait impl per resource; can't reuse an extraction against multiple sources |
| 2 | The trait only ever extracts the **first** match remotely (`scrape_first_erased`) | Listing pages (10 products, 50 links) are unscrapable remotely; `scrape_html_all` exists only offline |
| 3 | One selector → one output: no way to pull several fields from different parts of one document | Users shoehorn everything into a single `elem_into` or abandon the crate |
| 4 | `id_url(&self, id) -> String` forced an id-templated URL scheme | Non-templated URLs need a fake "id"; the map from ids to URLs is trivial with `format!` anyway |
| 5 | Errors carry almost no context (`HttpStatus(u16)`, `NoElementMatch` without selector) | Debugging a 10k-item batch means guessing which URL/selector failed |
| 6 | No politeness: no rate limiting, no `Retry-After` handling, no jitter cap | Gets users IP-banned; 429 responses silently parsed as HTML in v0.1 style |
| 7 | No response size cap, no charset handling beyond reqwest's text() | Decompression bombs OOM the process; ISO-8859-1 pages mangled |
| 8 | No caching | Re-runs re-download everything; the same URL twice in one batch is fetched twice |
| 9 | No streaming | 100k URLs must all finish before the first result is usable |
| 10 | Selector re-parsed per call for offline scrapes; no memoization | Wasted CPU in hot loops |
| 11 | Test suite measured server-side concurrency *including* socket teardown | `scrape_clamps_zero_concurrency_to_one` was inherently racy (observed failing) |

### Competitive landscape (why this niche, why now)

- **`scraper` crate**: excellent HTML parsing/CSS selection, zero fetching or
  extraction opinion. scrpr's job is the layer *above* it.
- **`spider`/`crawl` crates**: full async crawlers, heavy, opinionated about
  queueing; no typed extraction story.
- **Python Scrapy / BeautifulSoup**: the gold standard for DX, but not Rust,
  not typed, GIL-bound for parsing.
- **No Rust crate** offers *typed extraction into your own structs* +
  *polite bounded fetching* in one tiny dependency-free-ish package. That is
  scrpr v0.3's position: **"serde for HTML pages, with a polite HTTP client
  attached"**.

## Brainstorm

**DX (the headline features)**

1. **`Extract` trait**: `fn extract(page: &Page) -> Result<Self>` — serde-like
   "your type, your fields, one impl", decoupled from how the page arrived
   (offline string, cache, or network).
2. **`Extractor<T>` combinator DSL** (`field::text`, `field::attr`,
   `field::attr_url`, `field::json_ld`, ...): composable, cloneable,
   `map`/`try_map`/`parse::<F>()`/`optional()`/`or()` transforms, and tuple
   extraction (`(&title, &price).get_all(page)?`) for ad-hoc fields.
3. **`Page` handle**: parsed document + source URL with `select`,
   `select_first`, `links` (absolute-ized), `title`, `meta`, `meta_property`,
   `scripts`, `json_ld`, `resolve`.
4. **Rich serializable errors** with context: url, status, selector,
   attribute, parse target, body-limit — every variant tells you *where* and
   *what*.
5. **Streaming batch API**: `scrape_stream` yields `(index, result)` as pages
   complete; `scrape` keeps the ordered-Vec contract.
6. **Urls in, data out**: drop the id concept; batch APIs take any iterator of
   URLs. Id-templating is a one-line `format!` at the call site.

**Performance**

1. One shared, pooled `reqwest::Client` per `Scraper` (kept from v0.2).
2. **Process-wide selector cache** (`Arc<Selector>`, bounded, thread-safe):
   the same selector string is parsed exactly once per process, across all
   `Page` and `Extractor` uses.
3. **`Arc<Page>` + optional TTL cache**: fetched pages are shared without
   re-parsing; cache hits skip both HTTP and HTML parsing.
4. **Capped streaming body reads**: `Response::chunk()` loop with a hard byte
   limit (default 10 MiB) instead of unbounded `text()`.
5. Auto-decompression (gzip/brotli/zstd/deflate) via reqwest features.
6. `buffered(concurrency)` fan-out kept; `scrape_stream` enables consumers to
   start processing before the batch finishes.

**Politeness / robustness**

1. Retries on network errors, timeouts, 408/429/5xx with exponential backoff
   capped by `max_retry_delay`.
2. `Retry-After` honored (seconds form) when `respect_retry_after(true)`.
3. `rate_limit(max_rps)` / `min_request_interval(Duration)` — a token-slot
   pacing gate every request (including retries) must pass.
4. Optional TTL cache (`cache_capacity` + `cache_ttl`), opt-in.
5. Timeouts: total + connect + read. Configurable redirects, cookie store,
   proxies, TLS-tolerance, default headers, default User-Agent identifying
   the scraper.

**KISS / what was deliberately rejected**

1. **No proc-macro derive** (`#[derive(Scrape)]`): a second crate + syn/quote
   weight for what a 10-line `impl Extract` expresses; the tuple `FieldSet`
   impls cover the ad-hoc case without macros.
2. **No `governor`/`rand` deps**: the pacing gate is 20 lines; deterministic
   capped backoff is testable and jitter-free by design.
3. **No robots.txt, no browser/JS rendering, no crawling frontier**: out of
   scope for a *tiny* extraction toolkit; `Page::links` gives the raw
   material for users to build frontier logic.
4. **No unordered stream variant**: ordering is the sane default; consumers
   needing pipelining get it from the stream anyway.
5. Keep the dependency tree lean: only additions are `encoding_rs` (charset),
   `tracing` (observability), `serde_json` behind the default-on `json`
   feature.

## Implementation decisions

- **Closure-based `Extractor`**: `Extractor<T>` is a label + `Arc<dyn Fn(&Page)
  -> Result<T>>`. Combinators wrap closures; vtable cost is noise next to
  HTML parsing, and the DSL stays 100% allocation-trivial to extend.
- **Selector cache**: `Mutex<HashMap<Box<str>, Entry>>` behind `OnceLock`,
  bounded at 1024 entries (cleared when full — cheap, keeps memory flat).
  Errors are cached too: an invalid selector string costs one parse, ever.
- **Charset decoding**: `encoding_rs` driven by the `Content-Type` charset
  parameter, UTF-8 default, BOM-aware.
- **`#[non_exhaustive]` `ScrapeError`**: new failure modes can be added
  without breaking matchers; serde round-trip preserved.
- **Deterministic concurrency tests**: the test server now counts a request
  as "active" only while the *handler* runs (not during socket teardown),
  which removed the race that made
  `scrape_clamps_zero_concurrency_to_one` flaky in v0.2.
- **Chunked-encoding test path**: the hand-rolled server gained chunked
  responses to exercise the mid-stream body cap without any new dev-deps.
- **100% offline testability** kept: every feature is tested against the
  local hand-rolled HTTP server; doctests never touch the network.

## Migration from 0.2 (breaking)

| 0.2 | 0.3 |
|-----|-----|
| `impl RemoteScrapable` (`id_url` + `res_selector` + `elem_into`) | `impl Extract` (`fn extract(page: &Page)`) |
| `RemoteScraper` / `RemoteScraperBuilder` | `Scraper` / `ScraperBuilder` |
| `scraper.scrape(&resource, ids)` | `scraper.scrape::<E, _, _>(urls)` |
| `remote_scrape(&resource, ids)` | `Scraper::new()?.scrape::<E, _, _>(..)` or free `fetch_as` |
| `scrape_html(&resource, html)` | `scrape_html::<E>(html)` |
| `scrape_html_all(&resource, html)` | `field::text_all(sel).get(&page)` / `page.select(sel)` |
| `RemoteScrapeError::HttpStatus(u16)` | `ScrapeError::Http { status, url }` |
| `ElementExt::required_attr(name)` | kept; error now carries `selector: Option<String>` context |

## Constraints discovered during implementation

Two findings from the 0.3 implementation are worth recording for posterity:

1. **`scraper::Html` is neither `Send` nor `Sync`.** Since `scraper` 0.29 the
   underlying `markup5ever` text nodes use non-atomic `tendril`s ("akin to
   `Rc`"), so `Html` — and therefore `Page` — cannot cross threads at all.
   Consequences baked into the design:
   - batch/stream APIs return **your extracted types** (which are `Send` by
     the `Extract` supertrait), never `Page` values, so whole pipelines stay
     spawnable;
   - the page cache stores **decoded html** (a `String`), re-parsing on hit
     — the HTTP round trip, not the parse, was the expensive part;
   - a `Page`-typed batch API (`fetch_many`, `page_stream`) was considered
     and rejected: its future/stream could never be `Send`.
2. **`futures::StreamExt::then` yields outputs, not futures.**
   `stream.then(closure).buffered(n)` silently degrades to unbounded
   pipelining (the `Then` combinator awaits internally). The bounded
   concurrency fan-out must be `stream.map(closure-returning-future)
   .buffered(n)` — the same shape v0.2 used with `stream::iter`.

## Verification gates (0.3)

- `cargo fmt --all --check`
- `cargo clippy --all-targets -- -D warnings` with `pedantic` enabled in
  `[lints]`
- `cargo test` — 89 unit/integration tests + 16 doctests, all offline
