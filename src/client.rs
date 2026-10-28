use std::time::Duration;

use reqwest::Method;
use reqwest::blocking::Client as Http;
use reqwest::header::ACCEPT;
use serde_json::{Map, Value, json};

use crate::cli::PageArgs;
use crate::error::{AppError, Result};

/// Redmine refuses page sizes above 100.
const MAX_PAGE: u32 = 100;

pub struct Client {
    http: Http,
    base: String,
    key: String,
    pub dry_run: bool,
}

impl Client {
    pub fn new(base: &str, key: String, dry_run: bool) -> Result<Self> {
        let timeout = std::env::var("REDMINE_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        let http = Http::builder()
            .user_agent(concat!("redmine-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(timeout))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            key,
            dry_run,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
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
        let url = self.url(path);
        if self.dry_run && method != Method::GET {
            let query: Map<String, Value> = query
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            return Ok(Some(json!({
                "dry_run": true,
                "method": method.as_str(),
                "url": url,
                "query": query,
                "body": body,
            })));
        }

        let mut req = self
            .http
            .request(method.clone(), &url)
            .header("X-Redmine-API-Key", &self.key)
            .header(ACCEPT, "application/json")
            .query(query);
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send()?;
        let status = resp.status().as_u16();
        let text = resp.text()?;
        if !(200..300).contains(&status) {
            return Err(api_error(status, &method, path, &text));
        }
        if text.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        ))
    }

    /// GET a paginated collection. With `--all`, follows `total_count` until exhausted.
    pub fn list(
        &self,
        path: &str,
        key: &str,
        query: Vec<(String, String)>,
        page: &PageArgs,
    ) -> Result<Value> {
        let with_page = |limit: u32, offset: u32| {
            let mut q = query.clone();
            q.push(("limit".into(), limit.to_string()));
            q.push(("offset".into(), offset.to_string()));
            q
        };
        if !page.all {
            return self.get(path, &with_page(page.limit.min(MAX_PAGE), page.offset));
        }

        let mut items = Vec::new();
        let mut offset = page.offset;
        let total = loop {
            let resp = self.get(path, &with_page(MAX_PAGE, offset))?;
            let batch = resp
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = resp.get("total_count").and_then(Value::as_u64);
            let n = batch.len() as u32;
            items.extend(batch);
            offset += n;
            match total {
                Some(t) if n > 0 && u64::from(offset) < t => continue,
                _ => break total,
            }
        };
        let count = items.len();
        Ok(json!({
            key: items,
            "total_count": total.unwrap_or(count as u64),
            "offset": page.offset,
            "limit": count,
        }))
    }
}

fn api_error(status: u16, method: &Method, path: &str, body: &str) -> AppError {
    let details: Vec<String> = serde_json::from_str::<Value>(body)
        .ok()
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
