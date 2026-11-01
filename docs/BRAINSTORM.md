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
