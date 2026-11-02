//! Process-wide CSS selector cache.
//!
//! The same handful of selector strings is typically used against thousands
//! of pages, so parsing them more than once per process is wasted work.
//! [`cached_selector`] memoizes parse results — successes *and* failures —
//! in a bounded, thread-safe map and hands out [`Arc`] clones.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use scraper::Selector;

use crate::error::ScrapeError;

/// Maximum number of memoized selectors before the cache is cleared.
///
/// Clearing (instead of LRU eviction) keeps the structure trivial while
/// bounding memory; a bulk run with thousands of one-shot selectors simply
/// restarts the cache periodically.
const CACHE_CAPACITY: usize = 1024;

struct Entry {
    selector: Option<Arc<Selector>>,
    error: Option<String>,
}

/// Bounded, thread-safe selector cache. Unit-testable in isolation.
#[derive(Default)]
struct SelectorCache {
    map: HashMap<Box<str>, Entry>,
}

impl SelectorCache {
    fn get(&self, selector: &str) -> Option<Result<Arc<Selector>, ScrapeError>> {
        self.map.get(selector).map(|entry| match &entry.selector {
            Some(parsed) => Ok(Arc::clone(parsed)),
            None => Err(ScrapeError::selector(
                selector,
                entry.error.clone().unwrap_or_default(),
            )),
        })
    }

    fn insert(&mut self, selector: &str) -> Result<Arc<Selector>, ScrapeError> {
        if self.map.len() >= CACHE_CAPACITY {
            self.map.clear();
        }
        match Selector::parse(selector) {
            Ok(parsed) => {
                let shared = Arc::new(parsed);
                self.map.insert(
                    Box::from(selector),
                    Entry {
                        selector: Some(Arc::clone(&shared)),
                        error: None,
                    },
                );
                Ok(shared)
            }
            Err(err) => {
                let message = err.to_string();
                self.map.insert(
                    Box::from(selector),
                    Entry {
                        selector: None,
                        error: Some(message.clone()),
                    },
                );
                Err(ScrapeError::selector(selector, message))
            }
        }
    }
}

fn global() -> &'static Mutex<SelectorCache> {
    static CACHE: OnceLock<Mutex<SelectorCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(SelectorCache::default()))
}

fn lock(cache: &Mutex<SelectorCache>) -> MutexGuard<'_, SelectorCache> {
    // A panic while holding the lock must not poison every future scrape.
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Returns the memoized parse of `selector`.
///
/// The first call parses (and caches); every later call anywhere in the
/// process returns an [`Arc`] clone of the same [`Selector`]. Invalid
/// selectors yield the same cached [`ScrapeError::Selector`] every time.
pub(crate) fn cached_selector(selector: &str) -> Result<Arc<Selector>, ScrapeError> {
    let mut cache = lock(global());
    if let Some(cached) = cache.get(selector) {
        return cached;
    }
    cache.insert(selector)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_selectors_share_one_parse() {
        let first = cached_selector("img.icon").unwrap();
        let second = cached_selector("img.icon").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn invalid_selectors_are_cached_too() {
        let first = cached_selector("<<<cache-invalid>>>").unwrap_err();
        let second = cached_selector("<<<cache-invalid>>>").unwrap_err();
        assert_eq!(first, second);
    }

    #[test]
    fn cache_clears_when_full() {
        let mut cache = SelectorCache::default();
        for i in 0..(CACHE_CAPACITY + 10) {
            let selector = format!("div.generated-{i}");
            cache.insert(&selector).unwrap();
        }
        // The cache was cleared on overflow, so it only holds the tail.
        assert!(cache.map.len() < 32);
        // And it still works after clearing.
        assert!(cache.insert("div.after-clear").is_ok());
    }

    #[test]
    fn cache_holds_both_successes_and_failures() {
        let mut cache = SelectorCache::default();
        assert!(cache.insert("p.ok").is_ok());
        assert!(cache.insert("p.bad <<<").is_err());
        assert!(cache.get("p.ok").unwrap().is_ok());
        assert!(cache.get("p.bad <<<").unwrap().is_err());
        assert!(cache.get("p.unknown").is_none());
    }
}
