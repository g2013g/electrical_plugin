//! Mouser Electronics part search — the live side of the parts-library
//! picker ("search the distributor catalog, then land a hit in the library").
//!
//! Unlike Octopart/Nexar, Mouser's Search API needs **no OAuth**: a single
//! free Search API key (a UUID) rides along as a query parameter, so the
//! per-part billing that made the Nexar free tier painful does not apply. The
//! key comes from the environment (`MOUSER_API_KEY`, or `MOUSER_SEARCH_API_KEY`
//! as an alias) at dispatch time and is never stored on disk or in a drawing.
//!
//! Flow:
//! 1. `POST api.mouser.com/api/v1/search/keyword?apiKey=<key>` with a
//!    `SearchByKeywordRequest` body → numbered part hits.
//!
//! The transport is a trait so request building and response parsing are
//! unit-testable without network.

use serde_json::Value;


pub const MOUSER_SEARCH_URL: &str = "https://api.mouser.com/api/v1/search/keyword";

/// Environment variable carrying the Mouser Search API key.
pub const ENV_API_KEY: &str = "MOUSER_API_KEY";
/// Accepted alias for the same key (the name Mouser's own samples use).
pub const ENV_API_KEY_ALT: &str = "MOUSER_SEARCH_API_KEY";

#[derive(Debug, Clone, PartialEq)]
pub enum MouserError {
    // Display is hand-rolled (thiserror was dropped in the port): the plugin
    // only ever shows these strings.
        Http(String),

        Api(String),

        BadResponse,
}

/// Mouser Search API key (mouser.com → My Mouser → Account → APIs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouserKey {
    pub api_key: String,
}

impl std::fmt::Display for MouserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl MouserKey {
    /// Read the key from `MOUSER_API_KEY` (or `MOUSER_SEARCH_API_KEY`). `Err`
    /// names the variable — the key is never stored in the library or the
    /// drawing.
    pub fn from_env() -> Result<Self, String> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(api_key) = get(ENV_API_KEY).or_else(|| get(ENV_API_KEY_ALT)) {
            return Ok(MouserKey { api_key });
        }
        Err(format!(
            "environment variable {ENV_API_KEY} not set (or {ENV_API_KEY_ALT})"
        ))
    }
}

/// One search hit, in library-ready terms.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PartHit {
    /// Mouser's own (prefixed) part number, e.g. `571-DTM06-08SA`. Lands in
    /// the `mouser_pn` attribute.
    pub mouser_pn: String,
    /// Manufacturer part number — the canonical library part number.
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    /// Unit price for quantity 1, when Mouser returned pricing. Lands in the
    /// `unit_cost` attribute so the BOM can roll costs up.
    pub price: Option<f64>,
}

/// The blocking HTTPS call, abstracted for tests.
pub trait Http {
    /// POST a JSON body, return the response body.
    fn post_json(&self, url: &str, body: &str) -> Result<String, String>;
}

/// Real transport (ureq, rustls).
pub struct RealHttp;

impl Http for RealHttp {
    fn post_json(&self, url: &str, body: &str) -> Result<String, String> {
        let response = ureq::post(url)
            .timeout(std::time::Duration::from_secs(30))
            .set("Content-Type", "application/json")
            .set("Accept", "application/json")
            .send_string(body)
            .map_err(|e| e.to_string())?;
        response.into_string().map_err(|e| e.to_string())
    }
}

/// Search Mouser with the user's key.
pub fn search(key: &MouserKey, query: &str, limit: usize) -> Result<Vec<PartHit>, MouserError> {
    search_with(&RealHttp, key, query, limit)
}

/// [`search`] against an injectable transport.
pub fn search_with(
    http: &dyn Http,
    key: &MouserKey,
    query: &str,
    limit: usize,
) -> Result<Vec<PartHit>, MouserError> {
    let url = format!("{MOUSER_SEARCH_URL}?apiKey={}", key.api_key);
    let body = serde_json::json!({
        "SearchByKeywordRequest": {
            "keyword": query,
            "records": limit.clamp(1, 50),
            "startingRecord": 0,
            "searchOptions": "None",
            "searchWithYourSignUpLanguage": "false",
        }
    });
    let text = http
        .post_json(&url, &body.to_string())
        .map_err(MouserError::Http)?;
    parse_search_response(&text)
}

/// Parse the keyword-search response. A non-empty `Errors` array is surfaced
/// with its message; an empty result set is simply no hits.
pub fn parse_search_response(text: &str) -> Result<Vec<PartHit>, MouserError> {
    let parsed: Value = serde_json::from_str(text).map_err(|_| MouserError::BadResponse)?;
    if let Some(errors) = parsed.get("Errors").and_then(Value::as_array) {
        let message = errors
            .iter()
            .filter_map(|e| match e {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o
                    .get("Message")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("; ");
        if !message.is_empty() {
            return Err(MouserError::Api(message));
        }
    }
    let parts = parsed
        .get("SearchResults")
        .and_then(|s| s.get("Parts"))
        .and_then(Value::as_array)
        .ok_or(MouserError::BadResponse)?;
    Ok(parts
        .iter()
        .filter_map(|p| {
            let mpn = str_of(p, "ManufacturerPartNumber").unwrap_or_default();
            let mouser_pn = str_of(p, "MouserPartNumber").unwrap_or_default();
            if mpn.is_empty() && mouser_pn.is_empty() {
                return None;
            }
            Some(PartHit {
                mouser_pn,
                mpn,
                manufacturer: str_of(p, "Manufacturer").unwrap_or_default(),
                description: str_of(p, "Description").unwrap_or_default(),
                price: first_price(p),
            })
        })
        .collect())
}

/// The unit price for quantity 1 from the hit's `PriceBreaks` array —
/// `[{"Quantity": "1", "Price": "0.53"}, …]` in the keyword-search response.
/// Absent or unparseable pricing stays `None` (an unpriced library part).
fn first_price(part: &Value) -> Option<f64> {
    let breaks = part.get("PriceBreaks")?.as_array()?;
    let best = breaks
        .iter()
        .filter_map(|b| {
            let qty = b.get("Quantity")?.as_str()?.trim().parse::<u32>().ok()?;
            Some((qty, b.get("Price")?.as_str()?.trim().to_string()))
        })
        .min_by_key(|(qty, _)| *qty)?;
    best.1.trim_start_matches('$').parse::<f64>().ok().filter(|p| *p > 0.0)
}

fn str_of(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ── result cache ──────────────────────────────────────────────────────
// Mouser is not per-part billed, but a cache still saves the round trip on
// repeats and lets a search survive a transient network failure. Entries
// live beside the library sidecar, keyed by normalized query + limit, and
// expire after 30 days so stale hits are not served forever.

/// Cache entry: query key → hits + unix-seconds save time.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    hits: Vec<PartHit>,
    saved_at_secs: u64,
}

#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct CacheFile {
    queries: std::collections::BTreeMap<String, CacheEntry>,
}

/// Default cache path: beside the library sidecar
/// (`%APPDATA%/OpenCADStudio/mouser-cache.json`).
pub fn default_cache_path() -> std::path::PathBuf {
    // Beside the EL plugin's config area (`%APPDATA%/OpenCADStudio`).
    std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::path::PathBuf::new)
        .join("OpenCADStudio")
        .join("el-mouser-cache.json")
}

fn cache_key(query: &str, limit: usize) -> String {
    format!("{}|{limit}", query.trim().to_lowercase())
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const CACHE_TTL_SECS: u64 = 30 * 24 * 3600;

fn load_cache(path: &std::path::Path) -> CacheFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_cache(path: &std::path::Path, cache: &CacheFile) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Write temp then rename so a crash mid-write cannot leave a truncated
    // cache (the library sidecar does the same).
    if let Ok(text) = serde_json::to_string(cache) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// [`search`] with the disk cache: a fresh hit list is stored, a cached one
/// (same normalized query, < 30 days old) is served without touching the
/// network. A failed network search still consults a stale entry as a
/// fallback — better old hits than none.
pub fn cached_search(
    key: &MouserKey,
    query: &str,
    limit: usize,
    cache_path: &std::path::Path,
) -> Result<Vec<PartHit>, MouserError> {
    cached_search_with(&RealHttp, key, query, limit, cache_path, unix_secs())
}

/// [`cached_search`] with injectable transport and clock for tests.
pub fn cached_search_with(
    http: &dyn Http,
    key: &MouserKey,
    query: &str,
    limit: usize,
    cache_path: &std::path::Path,
    now_secs: u64,
) -> Result<Vec<PartHit>, MouserError> {
    let key_str = cache_key(query, limit);
    let mut cache = load_cache(cache_path);
    if let Some(entry) = cache.queries.get(&key_str) {
        if now_secs.saturating_sub(entry.saved_at_secs) < CACHE_TTL_SECS {
            return Ok(entry.hits.clone());
        }
    }
    match search_with(http, key, query, limit) {
        Ok(hits) => {
            cache.queries.insert(
                key_str,
                CacheEntry {
                    hits: hits.clone(),
                    saved_at_secs: now_secs,
                },
            );
            save_cache(cache_path, &cache);
            Ok(hits)
        }
        Err(e) => {
            if let Some(entry) = cache.queries.get(&key_str) {
                return Ok(entry.hits.clone());
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockHttp {
        last_request: std::sync::Mutex<Option<(String, String)>>,
        response: String,
    }

    impl MockHttp {
        fn new(response: &str) -> Self {
            MockHttp {
                last_request: std::sync::Mutex::new(None),
                response: response.into(),
            }
        }
    }

    impl Http for MockHttp {
        fn post_json(&self, url: &str, body: &str) -> Result<String, String> {
            *self.last_request.lock().unwrap() = Some((url.to_string(), body.to_string()));
            Ok(self.response.clone())
        }
    }

    fn key() -> MouserKey {
        MouserKey {
            api_key: "key-123".into(),
        }
    }

    const SEARCH_OK: &str = r#"{"Errors":[],"SearchResults":{"NumberOfResult":3,"Parts":[
        {"MouserPartNumber":"571-DTM06-08SA","ManufacturerPartNumber":"DTM06-08SA","Manufacturer":"TE Connectivity","Description":"8-way plug"},
        {"MouserPartNumber":"538-43045-0400","ManufacturerPartNumber":"43045-0400","Manufacturer":"Molex","Description":""},
        {"MouserPartNumber":"","ManufacturerPartNumber":"","Manufacturer":"x","Description":"junk"}
    ]}}"#;

    #[test]
    fn search_sends_key_and_parses_hits() {
        let http = MockHttp::new(SEARCH_OK);
        let hits = search_with(&http, &key(), "dtm 8 way", 10).unwrap();

        // Request shape: key rides as a query parameter, body is a keyword query.
        let (url, body) = http.last_request.lock().unwrap().clone().unwrap();
        assert!(url.contains("apiKey=key-123"), "{url}");
        assert!(url.contains("/search/keyword"), "{url}");
        assert!(body.contains("\"keyword\":\"dtm 8 way\""), "{body}");
        assert!(body.contains("\"records\":10"), "{body}");

        // Usable hits only (the all-blank third part is dropped).
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].mpn, "DTM06-08SA");
        assert_eq!(hits[0].mouser_pn, "571-DTM06-08SA");
        assert_eq!(hits[0].manufacturer, "TE Connectivity");
        assert_eq!(hits[1].mpn, "43045-0400");
        assert!(hits[1].description.is_empty());
    }

    #[test]
    fn api_errors_surface_their_message() {
        let http = MockHttp::new(r#"{"Errors":[{"Message":"Invalid API key"}]}"#);
        let err = search_with(&http, &key(), "x", 5).unwrap_err();
        assert!(err.to_string().contains("Invalid API key"), "{err}");
    }

    #[test]
    fn malformed_response_is_bad_response() {
        let http = MockHttp::new(r#"{"Errors":[]}"#);
        assert!(matches!(
            search_with(&http, &key(), "x", 5),
            Err(MouserError::BadResponse)
        ));
    }

    #[test]
    fn hits_parse_the_manufacturer_and_part_number() {
        // EL's consumer reads the hit fields directly (MFG/CAT per device),
        // so the mapping test is: the fields we assign from are populated.
        let hits = parse_search_response(SEARCH_OK).unwrap();
        assert!(hits.len() >= 1);
        assert_eq!(hits[0].manufacturer, "TE Connectivity");
        assert!(!hits[0].mpn.is_empty());
    }

    #[test]
    fn cache_serves_repeats_without_network_and_expires() {
        let dir = std::env::temp_dir().join(format!("wh-mouser-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache_path = dir.join("mouser-cache.json");

        // Mock that FAILS after the first call: if the cache works, the
        // second search never reaches it.
        struct OnceHttp {
            calls: std::sync::Mutex<u32>,
        }
        impl Http for OnceHttp {
            fn post_json(&self, _: &str, _: &str) -> Result<String, String> {
                let mut c = self.calls.lock().unwrap();
                *c += 1;
                if *c > 1 {
                    return Err("network down".into());
                }
                Ok(SEARCH_OK.into())
            }
        }
        let http = OnceHttp {
            calls: std::sync::Mutex::new(0),
        };

        let key = MouserKey {
            api_key: "c".into(),
        };
        let first = cached_search_with(&http, &key, "dtm 8 way", 5, &cache_path, 1_000_000).unwrap();
        assert_eq!(first.len(), 2);
        // Repeat within TTL: served from the cache, no second network call.
        let second =
            cached_search_with(&http, &key, "DTM 8 WAY", 5, &cache_path, 1_000_000 + 1000).unwrap();
        assert_eq!(second, first);
        assert_eq!(*http.calls.lock().unwrap(), 1);

        // Past the TTL the cache re-searches (the mock fails) and the
        // stale-fallback still serves the old hits.
        let stale =
            cached_search_with(&http, &key, "dtm 8 way", 5, &cache_path, 1_000_000 + CACHE_TTL_SECS + 1)
                .unwrap();
        assert_eq!(stale, first);
        assert_eq!(*http.calls.lock().unwrap(), 2);

        // A query never searched has no fallback: the failure surfaces.
        let err = cached_search_with(&http, &key, "something new", 5, &cache_path, 1_000_000 + CACHE_TTL_SECS + 1);
        assert!(matches!(err, Err(MouserError::Http(_))), "{err:?}");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn cache_key_normalizes_query_and_limit() {
        assert_eq!(cache_key(" DTM 8 Way  ", 5), cache_key("dtm 8 way", 5));
        assert_ne!(cache_key("dtm", 5), cache_key("dtm", 10));
    }
}
