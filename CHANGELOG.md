# Changelog

All notable changes to this project will be documented in this file.

## [0.3.0] - 2026-11-03

### ⛰️ Features (breaking redesign)

- **Typed extraction**: new `Extract` trait (serde-style, one impl per domain
  type) plus `scrape_html`, `Scraper::fetch_as`, `Scraper::scrape` and
  `Scraper::scrape_stream` entry points.
- **Composable field DSL**: `field::{text, text_all, attr, attr_opt, attr_all,
  attr_url, inner_html, outer_html, links, page_url, title, meta,
  meta_property, json_ld}` extractors with `map` / `try_map` / `parse::<F>` /
  `optional` / `or` / `or_default` combinators and tuple extraction
  (`FieldSet`, up to 12 fields, owned or borrowed).
- **`Page` handle**: parsed document + source url with `select`,
  `select_first`, `links`, `title`, `meta`, `meta_property`, `scripts`,
  `json_ld` (behind the default `json` feature) and `resolve`.
- **Streaming batch API**: `scrape_stream` yields `(position, result)` pairs
  in input order with bounded concurrency.
- **Politeness**: `rate_limit` / `min_request_interval` pacing, capped
  exponential backoff, `Retry-After` honored on 429 (configurable),
  identifying default `User-Agent`.
- **Robustness**: response body size cap with capped streaming reads,
  charset decoding via `encoding_rs`, connect/read timeouts, redirect
  policy controls, cookie store, proxy support, default headers.
- **Optional page cache**: capacity + TTL bounded, deduplicates repeated
  urls, shared across `Scraper` clones.
- **Process-wide CSS selector cache** (`Arc<Selector>` memoization).
- **Rich serializable errors**: `ScrapeError` variants carry url, status,
  selector and attribute context; `#[non_exhaustive]`.

### 💥 Breaking

- Removed `RemoteScrapable`, `RemoteScraper`, `RemoteScraperBuilder`,
  `remote_scrape`; replaced by `Extract`, `Scraper`, `ScraperBuilder` and
  plain url iterators. See the migration table in the README.

### 🚜 Refactor

- Split the crate into `error`, `selector`, `page`, `extract` and `client`
  modules behind a curated public API plus a `prelude`.

### 🧪 Testing

- Deterministic bounded-concurrency measurement in the hand-rolled test
  server (handler-scoped activity window), new chunked-encoding and
  charset test paths, 89 integration tests + 16 doctests, all offline.

## [Unreleased]

### ⛰️ Features

- *(ci)* Add git-cliff changelog auto-generation workflow - ([32443e4](https://github.com/elcoosp/scrpr/commit/32443e47db7d7f55f435b035415ef07041e60b94))
- *(ci)* Add comprehensive CI pipeline with fmt, clippy, tests, MSRV and coverage - ([69ad7b7](https://github.com/elcoosp/scrpr/commit/69ad7b7a4b30846bb91f16847425b29e60c2eca9))
- [**breaking**] Bump to v0.2.0 with edition 2024, MSRV 1.88 and dependency overhaul - ([65f9704](https://github.com/elcoosp/scrpr/commit/65f970444b082a3cdda9ac059a5a0afffc92440b))
- Add typed RemoteScrapeError enum with serde and thiserror - ([9f74ee9](https://github.com/elcoosp/scrpr/commit/9f74ee9724bd5a02c97dbae8ada3519152ddd732))
- [**breaking**] Rewrite lib.rs with module structure and offline doctests - ([24927dd](https://github.com/elcoosp/scrpr/commit/24927ddac8db919ef54f0283bb01d6eb6efe3214))
- Implement RemoteScraper with bounded concurrency, retries and builder - ([f42d59e](https://github.com/elcoosp/scrpr/commit/f42d59e40d530931f292f7fcb50a99b674b93c45))
- Define RemoteScrapable trait and ElementExt helper trait - ([fead81a](https://github.com/elcoosp/scrpr/commit/fead81ac254439b54ea474eebede76beba6cb2e9))

### 🐛 Bug Fixes

- Renovate config - ([d3159e8](https://github.com/elcoosp/scrpr/commit/d3159e8e28b20f3c940d832447d620d6f51d624e))
- Release workflow only on main push - ([a1632c2](https://github.com/elcoosp/scrpr/commit/a1632c26a9ea7a2977f1be938d5f0e9b18a66fc1))
- Up deps & add acknowledgment - ([9c1b30c](https://github.com/elcoosp/scrpr/commit/9c1b30c29d205bfb11966635f9990e72a688031a))

### 🚜 Refactor

- *(ci)* Replace release-plz with git-cliff release pipeline - ([9a14910](https://github.com/elcoosp/scrpr/commit/9a14910ac7e9964d9c9e296ab1c4851d72fad0fb))

### 📚 Documentation

- Crate link fix - ([f02e980](https://github.com/elcoosp/scrpr/commit/f02e980298bcb53d92b38cc05b60162563848df3))
- Up readme - ([adaaf67](https://github.com/elcoosp/scrpr/commit/adaaf672fb4f9e74d84a4c1c6ddc32bd76e6e5b5))
- Fix double unreleased in changelog - ([72c7078](https://github.com/elcoosp/scrpr/commit/72c707802091d2d01f261a906de6131f8f5f224f))
- Regenerate changelog with git-cliff format - ([9ee4a66](https://github.com/elcoosp/scrpr/commit/9ee4a6659f62bab1cf1f6e76a510c9bd2343c334))
- Rewrite README with v0.2.0 features, examples and usage guide - ([c0fd411](https://github.com/elcoosp/scrpr/commit/c0fd411b56dcb4acd123cb68e07f5a835b5316c1))
- Add brainstorm and design rationale for v0.2.0 rewrite - ([f7d3e24](https://github.com/elcoosp/scrpr/commit/f7d3e24c857e6912bbaa9786299c867e0584e447))

### 🧪 Testing

- Add comprehensive offline test suite with 25+ test cases - ([2f3f878](https://github.com/elcoosp/scrpr/commit/2f3f87890a311df3675d4d76686a56c5e10f886b))
- Add hand-rolled HTTP test server with zero external dependencies - ([8b10338](https://github.com/elcoosp/scrpr/commit/8b10338c90a9dccb1c946349debdc66765c9b850))

### ⚙️ Miscellaneous Tasks

- Codecov-action v5 - ([cb9e31f](https://github.com/elcoosp/scrpr/commit/cb9e31f987546f46d19beff68d7d3cecdb9e0594))
- Readme double license badge - ([37e4182](https://github.com/elcoosp/scrpr/commit/37e41823863c67c5d6e104c70dd5746d19f37502))
- Crates.io version shield in readme - ([74810bd](https://github.com/elcoosp/scrpr/commit/74810bd6689dfb490494120567e27b5d4d84d3b1))
- Add license - ([ead4d83](https://github.com/elcoosp/scrpr/commit/ead4d83d90de326e6d29c15449f5a556c09ecb49))
- Up release-plz - ([7f9e759](https://github.com/elcoosp/scrpr/commit/7f9e759d3186832ef78e1195b1f7ebeb6eb97734))
- Use actions-rust-lang/setup-rust-toolchain - ([26f0c9a](https://github.com/elcoosp/scrpr/commit/26f0c9abc5159e092136191125263bbdd5745417))
- Readme - ([61f2c76](https://github.com/elcoosp/scrpr/commit/61f2c7636a6c5c66f26b52cea12624a19418eb38))


## [0.1.0]

### ⛰️ Features

- Impl remote_scrape fn using a RemoteScrapable and a bunch of ids to reqwest and scrape the res_selector then turn in into a Self::Output - ([ac652f5](https://github.com/elcoosp/scrpr/commit/ac652f50c3e0c2ecad70e627b6277c55ccd1c134))
- Serde De/Serializable RemoteScrapeError - ([6ee767e](https://github.com/elcoosp/scrpr/commit/6ee767e983e3764cdc6b9743fbe0d0723e2c1f9b))

### 🐛 Bug Fixes

- Missing Cargo.toml package metadata - ([87ea791](https://github.com/elcoosp/scrpr/commit/87ea7910f9626bbe1cdcb5e238be5ae6dde2c858))

### 📚 Documentation

- Readme & ci workflow - ([6e8aef9](https://github.com/elcoosp/scrpr/commit/6e8aef95011240ca57d9b7cfb2c4f51be3d95913))
- Readme gen - ([c4f555e](https://github.com/elcoosp/scrpr/commit/c4f555e1a0da4890cd1bdb97ef3b6fef931334c7))

### 🧪 Testing

- Remote_scrape fn & add PartialEq to crate Err - ([a762922](https://github.com/elcoosp/scrpr/commit/a76292259952d77e2331170109f2839122527629))

### ⚙️ Miscellaneous Tasks

- Workflow release with env - ([232e1da](https://github.com/elcoosp/scrpr/commit/232e1da43bf210711808db99ad8f0a6e71bd726f))
- Renovate - ([9a40dff](https://github.com/elcoosp/scrpr/commit/9a40dff1db2ec3e5192d21463ed97035f5e59a8f))
- Codecov - ([d4d8167](https://github.com/elcoosp/scrpr/commit/d4d8167f13a52c9705a38692ad00ebdd9db7f528))
- Rust nightly test - ([a28f343](https://github.com/elcoosp/scrpr/commit/a28f343bdd18ae90d4cef39bf22221df80071a93))
- Use Cllvm-args - ([bf5eedc](https://github.com/elcoosp/scrpr/commit/bf5eedca59827a5d4a5b308a01bfad966d13e41c))
- Rm Zprofile - ([e27f299](https://github.com/elcoosp/scrpr/commit/e27f2992eac477f6971bacfdfb0ad52f8d647972))
- Use https://github.com/mozilla/grcov?tab=readme-ov-file#example-how-to-generate-gcda-files-for-a-rust-project - ([ce56b00](https://github.com/elcoosp/scrpr/commit/ce56b000cfa1884cf62314665e9933927dd4f840))
- Readd toolchain - ([56587ea](https://github.com/elcoosp/scrpr/commit/56587ea83c1545c9720bc46e1ca8e2a03a0068d0))
- Missing with - ([10acbb8](https://github.com/elcoosp/scrpr/commit/10acbb8567723d557e833a967313f1cd9aa61de1))
- Rm -Zprofile see https://github.com/rust-lang/rust/pull/131829 - ([52b6525](https://github.com/elcoosp/scrpr/commit/52b6525033a876d24d49773d9da70ab40cd710ac))
- Split coverage action with tarpaulin - ([0fe666f](https://github.com/elcoosp/scrpr/commit/0fe666f665b0429c507f13fbf36e5145f894aaeb))

<!-- generated by git-cliff -->
