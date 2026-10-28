//! Name -> id resolution so callers can say `--status Closed` instead of `--status 5`.

use serde_json::Value;

use crate::client::Client;
use crate::error::{AppError, Result};

#[derive(Clone, Copy)]
pub enum Kind {
    Status,
    Tracker,
    Priority,
    Activity,
}

impl Kind {
    /// (endpoint, response key, label)
    pub fn endpoint(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Status => ("/issue_statuses.json", "issue_statuses", "status"),
            Self::Tracker => ("/trackers.json", "trackers", "tracker"),
            Self::Priority => (
                "/enumerations/issue_priorities.json",
                "issue_priorities",
                "priority",
            ),
            Self::Activity => (
                "/enumerations/time_entry_activities.json",
                "time_entry_activities",
                "activity",
            ),
        }
    }
}

fn is_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Accepts a numeric id as-is, otherwise matches the name case-insensitively.
pub fn resolve(client: &Client, kind: Kind, value: &str) -> Result<String> {
    let value = value.trim();
    if is_id(value) {
        return Ok(value.to_string());
    }
    let (path, key, label) = kind.endpoint();
    let list = client.get(path, &[])?;
    let items = list
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let wanted = value.to_lowercase();
    if let Some(item) = items.iter().find(|i| {
        i.get("name")
            .and_then(Value::as_str)
            .is_some_and(|n| n.to_lowercase() == wanted)
    }) {
        return Ok(item["id"].to_string());
    }
    let names: Vec<&str> = items
        .iter()
        .filter_map(|i| i.get("name").and_then(Value::as_str))
        .collect();
    Err(AppError::Usage(format!(
        "unknown {label} {value:?}; available: {}",
        names.join(", ")
    )))
}

/// Status filter for issue lists: open | closed | all | * | name | id.
pub fn status_filter(client: &Client, value: &str) -> Result<String> {
    match value.trim().to_lowercase().as_str() {
        "open" => Ok("open".into()),
        "closed" => Ok("closed".into()),
        "all" | "*" => Ok("*".into()),
        _ => resolve(client, Kind::Status, value),
    }
}

/// Accepts a numeric user id or "me" (resolved via /users/current.json).
pub fn resolve_user(client: &Client, value: &str) -> Result<String> {
    let value = value.trim();
    if is_id(value) {
        return Ok(value.to_string());
    }
    if value.eq_ignore_ascii_case("me") {
        let me = client.get("/users/current.json", &[])?;
        return me
            .pointer("/user/id")
            .map(Value::to_string)
            .ok_or_else(|| AppError::Response("missing user.id in /users/current.json".into()));
    }
    Err(AppError::Usage(format!(
        "user must be a numeric id or \"me\", got {value:?}"
    )))
}
