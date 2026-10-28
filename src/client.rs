use std::io::Read;
use std::time::Duration;

use reqwest::blocking::Client as Http;
use reqwest::header::{ACCEPT, LOCATION};
use reqwest::redirect::Policy;
use reqwest::{Method, Url};
use serde_json::{Map, Value, json};

use crate::cli::PageArgs;
use crate::error::{AppError, Result};

/// Redmine refuses page sizes above 100.
const MAX_PAGE: u32 = 100;
/// Upper bound on one response body; a hostile server must not exhaust memory.
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;
/// Default upper bound on items collected by `--all` (override: REDMINE_MAX_ITEMS).
const DEFAULT_MAX_ITEMS: usize = 10_000;
/// Default upper bound on the estimated memory of items collected by `--all`
/// (override: REDMINE_MAX_BYTES).
const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

pub struct Client {
    http: Http,
    base: String,
    base_url: Url,
    base_path: String,
    key: String,
    pub dry_run: bool,
}

impl Client {
    pub fn new(base: &str, key: String, dry_run: bool) -> Result<Self> {
        let timeout = std::env::var("REDMINE_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        let base_url =
            Url::parse(base).map_err(|e| AppError::Config(format!("invalid URL {base}: {e}")))?;
        let base_path = base_url.path().trim_end_matches('/').to_string();
        let http = Http::builder()
            .user_agent(concat!("redmine-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(timeout))
            .redirect(redirect_policy(base_path.clone()))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            base_url,
            base_path,
            key,
            dry_run,
        })
    }

    /// Joins `path` to the base URL and refuses anything that would leave it
    /// (`../other-app`, encoded separators), since the key goes with every request.
    fn url(&self, path: &str) -> Result<Url> {
        Url::parse(&format!("{}/{}", self.base, path.trim_start_matches('/')))
            .ok()
            .filter(|u| {
                u.origin() == self.base_url.origin() && within_base_path(&self.base_path, u.path())
            })
            .ok_or_else(|| {
                AppError::Usage(format!(
                    "path {path:?} leaves {}: the API key is only sent there",
                    self.base
                ))
            })
    }

    pub fn get(&self, path: &str, query: &[(String, String)]) -> Result<Value> {
        self.send(Method::GET, path, query, None)?
            .ok_or_else(|| AppError::Response(format!("GET {path} returned an empty body")))
    }

    /// Sends a request; returns `None` for empty bodies (e.g. 204 after PUT).
    /// With `dry_run`, write requests are not sent and a description is returned instead.
    pub fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<&Value>,
    ) -> Result<Option<Value>> {
        let url = self.url(path)?;
        if self.dry_run && method != Method::GET {
            let query: Map<String, Value> = query
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            return Ok(Some(json!({
                "dry_run": true,
                "method": method.as_str(),
                "url": url.as_str(),
                "query": query,
                "body": body,
            })));
        }

        let mut req = self
            .http
            .request(method.clone(), url)
            .header("X-Redmine-API-Key", &self.key)
            .header(ACCEPT, "application/json")
            .query(query);
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send()?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            let location = self.scrub(
                resp.headers()
                    .get(LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .unwrap_or("?")
                    .to_string(),
            );
            return Err(AppError::Api {
                status,
                message: format!(
                    "refused to follow redirect from {method} {path} to {location}: \
                     the API key is only sent to {}",
                    self.base
                ),
                details: Vec::new(),
            });
        }
        let text = read_limited(resp, MAX_BODY_BYTES)?;
        // Parse first, then mask: this catches keys hidden behind JSON escapes
        // and can never break the JSON structure.
        let mut value = (!text.trim().is_empty())
            .then(|| serde_json::from_str(&text).unwrap_or(Value::String(text)));
        if let Some(v) = value.as_mut() {
            self.scrub_value(v);
            remove_api_key_fields(v);
        }
        if !(200..300).contains(&status) {
            return Err(api_error(status, &method, path, value.as_ref()));
        }
        Ok(value)
    }

    /// Masks the key in every string of a response (JSON values and object keys,
    /// or a raw XML/HTML body).
    fn scrub_value(&self, value: &mut Value) {
        match value {
            Value::String(s) => *s = self.scrub(std::mem::take(s)),
            Value::Array(items) => items.iter_mut().for_each(|v| self.scrub_value(v)),
            Value::Object(map) => {
                *map = std::mem::take(map)
                    .into_iter()
                    .map(|(k, mut v)| {
                        self.scrub_value(&mut v);
                        (self.scrub(k), v)
                    })
                    .collect();
            }
            _ => {}
        }
    }

    fn scrub(&self, text: String) -> String {
        if self.key.is_empty() || !text.contains(&self.key) {
            return text;
        }
        text.replace(&self.key, "[REDACTED]")
    }

    /// GET a paginated collection. With `--all`, follows `total_count` until exhausted,
    /// stopping at REDMINE_MAX_ITEMS (then the result carries `"truncated": true`).
    pub fn list(
        &self,
        path: &str,
        key: &str,
        query: Vec<(String, String)>,
        page: &PageArgs,
    ) -> Result<Value> {
        let with_page = |limit: u32, offset: u64| {
            let mut q = query.clone();
            q.push(("limit".into(), limit.to_string()));
            q.push(("offset".into(), offset.to_string()));
            q
        };
        if !page.all {
            return self.get(
                path,
                &with_page(page.limit.min(MAX_PAGE), page.offset.into()),
            );
        }

        let max_items = env_limit("REDMINE_MAX_ITEMS", DEFAULT_MAX_ITEMS);
        let max_bytes = env_limit("REDMINE_MAX_BYTES", DEFAULT_MAX_BYTES);
        let mut items = Vec::new();
        let mut bytes = 0usize;
        let mut offset = u64::from(page.offset);
        let mut truncated = false;
        let total = loop {
            let mut resp = self.get(path, &with_page(MAX_PAGE, offset))?;
            let total = resp.get("total_count").and_then(Value::as_u64);
            // Move the page out rather than clone it: a page can be large.
            let batch = match resp.get_mut(key).map(Value::take) {
                Some(Value::Array(items)) => items,
                _ => Vec::new(),
            };
            let n = batch.len() as u64;
            offset += n;
            for item in batch {
                if items.len() >= max_items || bytes >= max_bytes {
                    truncated = true;
                    break;
                }
                bytes += mem_size(&item);
                items.push(item);
            }
            let more = total.is_some_and(|t| n > 0 && offset < t);
            if more && (items.len() >= max_items || bytes >= max_bytes) {
                truncated = true;
            }
            if !more || truncated {
                break total;
            }
        };
        let count = items.len();
        let mut result = json!({
            key: items,
            "total_count": total.unwrap_or(count as u64),
            "offset": page.offset,
            "limit": count,
        });
        if truncated {
            result["truncated"] = json!(true);
        }
        Ok(result)
    }
}

fn env_limit(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// Rough in-memory size of a parsed value. JSON size alone undercounts: `[]` is
/// 2 bytes of JSON but a whole `Value` once parsed.
fn mem_size(value: &Value) -> usize {
    std::mem::size_of::<Value>()
        + match value {
            Value::String(s) => s.len(),
            Value::Array(items) => items.iter().map(mem_size).sum(),
            Value::Object(map) => map.iter().map(|(k, v)| k.len() + mem_size(v)).sum(),
            _ => 0,
        }
}

fn read_limited(body: impl Read, limit: u64) -> Result<String> {
    let mut buf = Vec::new();
    body.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        return Err(AppError::Response(format!(
            "response body exceeds {} bytes",
            limit
        )));
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Removes every `api_key` field (e.g. /users/current.json returns the caller's).
fn remove_api_key_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("api_key");
            map.values_mut().for_each(remove_api_key_fields);
        }
        Value::Array(items) => items.iter_mut().for_each(remove_api_key_fields),
        _ => {}
    }
}

/// Follows redirects only within the original origin (or an http -> https:443 upgrade
/// on the same host) and under the base path. reqwest strips `Authorization` on
/// cross-host redirects but not custom headers, so following one would hand
/// X-Redmine-API-Key to that host.
fn redirect_policy(base_path: String) -> Policy {
    Policy::custom(move |attempt| {
        if attempt.previous().len() > 10 {
            return attempt.error("too many redirects");
        }
        let allowed = attempt.previous().first().is_some_and(|first| {
            same_origin_or_upgrade(first, attempt.url())
                && within_base_path(&base_path, attempt.url().path())
        });
        if allowed {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn same_origin_or_upgrade(from: &Url, to: &Url) -> bool {
    if from.host_str() != to.host_str() {
        return false;
    }
    match (from.scheme(), to.scheme()) {
        (a, b) if a == b => from.port_or_known_default() == to.port_or_known_default(),
        ("http", "https") => to.port_or_known_default() == Some(443),
        _ => false,
    }
}

/// `Url` has already resolved `..` and `%2e%2e` segments when this runs, but it
/// keeps encoded separators that a decoding proxy in front of Redmine would
/// resolve later, so any `%2f`, `%5c` or `%2e` left below the base path is refused.
fn within_base_path(base_path: &str, path: &str) -> bool {
    let Some(rest) = path.strip_prefix(base_path) else {
        return false;
    };
    let lower = rest.to_ascii_lowercase();
    (rest.is_empty() || rest.starts_with('/'))
        && !["%2f", "%5c", "%2e"].iter().any(|s| lower.contains(s))
}

fn api_error(status: u16, method: &Method, path: &str, body: Option<&Value>) -> AppError {
    let details: Vec<String> = body
        .and_then(|v| v.get("errors").and_then(Value::as_array).cloned())
        .map(|errs| {
            errs.iter()
                .map(|e| e.as_str().map_or_else(|| e.to_string(), str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let what = match status {
        401 => "authentication failed (check the API key and that the REST API is enabled)",
        403 => "permission denied",
        404 => "not found",
        422 => "validation failed",
        _ => "request failed",
    };
    let mut message = format!("{what}: {method} {path} returned HTTP {status}");
    if !details.is_empty() {
        message.push_str(": ");
        message.push_str(&details.join("; "));
    }
    AppError::Api {
        status,
        message,
        details,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(from: &str, to: &str) -> bool {
        same_origin_or_upgrade(&Url::parse(from).unwrap(), &Url::parse(to).unwrap())
    }

    #[test]
    fn redirects_stay_under_the_base_path() {
        let path = |u: &str| Url::parse(u).unwrap().path().to_string();
        assert!(within_base_path("", "/anything"));
        assert!(within_base_path("/redmine", &path("https://h/redmine")));
        assert!(within_base_path(
            "/redmine",
            &path("https://h/redmine/issues/1.json")
        ));
        assert!(!within_base_path(
            "/redmine",
            &path("https://h/redmine-evil/x")
        ));
        assert!(!within_base_path(
            "/redmine",
            &path("https://h/redmine/../other")
        ));
        assert!(!within_base_path(
            "/redmine",
            &path("https://h/redmine/%2e%2e/other")
        ));
        assert!(!within_base_path(
            "/redmine",
            &path("https://h/redmine/..%2F..%2fother")
        ));
        assert!(!within_base_path("", &path("https://h/a%5c..%5cother")));
        assert!(!within_base_path("", &path("https://h/a/%2E%2e./x")));
        // Encodings are matched case-insensitively.
        assert!(!within_base_path("/r", &path("https://h/r/..%2F..%2Fx")));
        assert!(!within_base_path("/r", &path("https://h/r/a%5Cb")));
        assert!(!within_base_path("/r", &path("https://h/r/a%2Eb")));
        // Only the part below the base path is checked.
        assert!(within_base_path("/r%2e", &path("https://h/r%2e/x")));
    }

    #[test]
    fn mem_size_counts_nodes_keys_and_strings() {
        let node = std::mem::size_of::<Value>();
        assert_eq!(mem_size(&json!([])), node);
        assert_eq!(mem_size(&json!([[], []])), 3 * node);
        assert_eq!(mem_size(&json!({"abcd": "xy"})), 2 * node + 4 + 2);
    }

    #[test]
    fn read_limited_rejects_oversized_bodies() {
        assert_eq!(read_limited(&b"12345"[..], 5).unwrap(), "12345");
        assert!(read_limited(&b"123456"[..], 5).is_err());
    }

    #[test]
    fn removes_nested_api_key_fields() {
        let mut v = json!({"user": {"id": 1, "api_key": "secret", "x": [{"api_key": "s2"}]}});
        remove_api_key_fields(&mut v);
        assert_eq!(v, json!({"user": {"id": 1, "x": [{}]}}));
    }

    #[test]
    fn redirects_stay_on_the_original_origin() {
        assert!(ok("https://r.example/a", "https://r.example/b"));
        assert!(ok("http://r.example/a", "https://r.example/a"));
        assert!(!ok("https://r.example/a", "http://r.example/a"));
        assert!(!ok("https://r.example/a", "https://evil.example/a"));
        assert!(!ok(
            "https://r.example/a",
            "https://r.example.evil.example/a"
        ));
        assert!(!ok("https://r.example/a", "https://r.example:8443/a"));
        assert!(!ok("http://r.example:9101/a", "https://r.example:9102/a"));
        assert!(ok("http://r.example:8080/a", "https://r.example/a"));
    }
}
