mod api;
mod auth;
mod issue;
mod lookup;
mod meta;
mod project;
mod schema;
mod time;
mod user;

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use serde_json::Value;

use crate::cli::{Cli, Command};
use crate::context::Context;
use crate::error::{AppError, Result};
use crate::output::Output;

pub fn run(cli: Cli, out: &Output) -> Result<()> {
    let ctx = Context::new(&cli.global, out.clone());
    match cli.command {
        Command::Auth(cmd) => auth::run(&ctx, cmd),
        Command::Issue(cmd) => issue::run(&ctx, cmd),
        Command::Project(cmd) => project::run(&ctx, cmd),
        Command::User(cmd) => user::run(&ctx, cmd),
        Command::Time(cmd) => time::run(&ctx, cmd),
        Command::Meta(cmd) => meta::run(&ctx, cmd),
        Command::Api(args) => api::run(&ctx, args),
        Command::Schema(args) => schema::run(&ctx, args),
    }
}

pub(crate) fn parse_kv(s: &str) -> Result<(String, String)> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
        .filter(|(k, _)| !k.is_empty())
        .ok_or_else(|| AppError::Usage(format!("expected KEY=VALUE, got {s:?}")))
}

/// Inline text wins; otherwise read the file, with "-" meaning stdin.
pub(crate) fn read_text(inline: Option<String>, file: Option<&Path>) -> Result<Option<String>> {
    if inline.is_some() {
        return Ok(inline);
    }
    match file {
        None => Ok(None),
        Some(p) if p == Path::new("-") => {
            let mut s = String::new();
            io::stdin().read_to_string(&mut s)?;
            Ok(Some(s))
        }
        Some(p) => fs::read_to_string(p)
            .map(Some)
            .map_err(|e| AppError::Io(format!("{}: {e}", p.display()))),
    }
}

/// Unwraps Redmine's `{"issue": {...}}` envelope.
pub(crate) fn unwrap_key(mut value: Value, key: &str) -> Result<Value> {
    value
        .get_mut(key)
        .map(Value::take)
        .ok_or_else(|| AppError::Response(format!("missing {key:?} in response")))
}

/// Query filters in Redmine's long form (`f[]=field&op[field]=..&v[field][]=..`).
/// Redmine understands it for issues since 1.2 and for time entries since 2.3,
/// while some servers silently ignore the short `field=value` form and return
/// everything.
/// Redmine's query operators that take values, and those that take none.
const BINARY_OPS: &[&str] = &[
    "=", "!", "^", "$", ">=", "<=", "><", "<t+", ">t+", "><t+", "t+", "<t-", ">t-", "><t-", "t-",
    "~", "!~", "*~", "=p", "=!p", "!p",
];
const UNARY_OPS: &[&str] = &[
    "o", "c", "*", "!*", "nd", "t", "ld", "nw", "w", "lw", "l2w", "nm", "m", "lm", "y", "*o", "!o",
    "ev", "!ev", "cf",
];

#[derive(Default)]
pub(crate) struct Filters(Vec<(String, String, Vec<String>)>);

impl Filters {
    /// Adds a filter, replacing any earlier one on the same field.
    pub fn set(&mut self, field: &str, op: &str, values: Vec<String>) {
        self.0.retain(|(f, _, _)| f != field);
        self.0.push((field.to_string(), op.to_string(), values));
    }

    pub fn has(&self, field: &str) -> bool {
        self.0.iter().any(|(f, _, _)| f == field)
    }

    /// Parses `-f "FIELD OP [VALUE[|VALUE...]]"`, e.g. `updated_on >= 2026-01-01`.
    pub fn set_expression(&mut self, expr: &str) -> Result<()> {
        let usage = || {
            AppError::Usage(format!(
                "expected a filter like \"FIELD OP [VALUE]\" (e.g. \"updated_on >= 2026-01-01\", \
                 \"cf_3 = a|b\", \"due_date !*\"), got {expr:?}"
            ))
        };
        let (field, rest) = expr
            .trim()
            .split_once(char::is_whitespace)
            .ok_or_else(usage)?;
        let rest = rest.trim_start();
        let (op, value) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(op, v)| (op, v.trim()));
        if !field
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
        {
            return Err(usage());
        }
        // Redmine ignores an unknown operator, so `cf_3 =foo` (a missing space)
        // would silently filter nothing; check the operator and its arity.
        match (BINARY_OPS.contains(&op), UNARY_OPS.contains(&op)) {
            (true, _) if value.is_empty() => return Err(usage()),
            (_, true) if !value.is_empty() => return Err(usage()),
            (false, false) => return Err(usage()),
            _ => {}
        }
        let values = if value.is_empty() {
            Vec::new()
        } else {
            value.split('|').map(str::to_string).collect()
        };
        self.set(field, op, values);
        Ok(())
    }

    pub fn into_query(self) -> Vec<(String, String)> {
        if self.0.is_empty() {
            return Vec::new();
        }
        let mut q = vec![("set_filter".to_string(), "1".to_string())];
        for (field, op, values) in self.0 {
            q.push(("f[]".into(), field.clone()));
            q.push((format!("op[{field}]"), op));
            for v in values {
                q.push((format!("v[{field}][]"), v));
            }
        }
        q
    }
}

/// A user filter value: a numeric id or "me" (Redmine resolves "me" itself).
pub(crate) fn user_value(value: &str) -> Result<String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("me") {
        return Ok("me".into());
    }
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(value.into());
    }
    Err(AppError::Usage(format!(
        "user must be a numeric id or \"me\", got {value:?}"
    )))
}

/// A project id or identifier that is safe to place in a URL path segment.
pub(crate) fn project_segment(id: &str) -> Result<&str> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(AppError::Usage(format!(
            "project must be a numeric id or identifier ([A-Za-z0-9_-]), got {id:?}"
        )));
    }
    Ok(id)
}

/// Numeric ids become JSON numbers; identifiers stay strings.
pub(crate) fn id_value(s: String) -> Value {
    s.parse::<u64>()
        .map(Value::from)
        .unwrap_or(Value::String(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(e: &str) -> Result<Vec<(String, String)>> {
        let mut f = Filters::default();
        f.set_expression(e)?;
        Ok(f.into_query())
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn filter_expressions_become_long_form() {
        assert_eq!(
            expr("updated_on >= 2026-01-01").unwrap(),
            pairs(&[
                ("set_filter", "1"),
                ("f[]", "updated_on"),
                ("op[updated_on]", ">="),
                ("v[updated_on][]", "2026-01-01"),
            ])
        );
        // Values keep inner spaces; `|` separates several values; unary operators take none.
        assert_eq!(
            expr("  subject   ~  vpn down|x ").unwrap()[3..],
            pairs(&[("v[subject][]", "vpn down"), ("v[subject][]", "x")])[..]
        );
        assert_eq!(expr("due_date !*").unwrap().len(), 3);
        for good in [
            "cf.x = 1",
            "status_id o",
            "start_date >< 2026-01-01|2026-02-01",
            "due_date <t+ 3",
        ] {
            assert!(expr(good).is_ok(), "{good:?}");
        }
        for bad in [
            "cf_3=foo",
            "updated_on=>=2026-01-01",
            "",
            "a/b = 1",
            "f[] = x",
            "a[ = 1",
            // A missing space glues the value onto the operator, which Redmine ignores.
            "cf_3 =foo",
            "updated_on >=2026-01-01",
            "x == 1",
            // Binary operators need a value.
            "subject ~",
            "cf_3 =",
        ] {
            assert!(expr(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn project_segments_are_plain_identifiers() {
        for good in ["web", "web-1_x", "42"] {
            assert_eq!(project_segment(good).unwrap(), good);
        }
        for bad in ["", "a/b", "a.b", "a?b", "a#b", "a%2fb", "a\\b", "a b", ".."] {
            assert!(project_segment(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn later_filters_replace_earlier_ones() {
        let mut f = Filters::default();
        f.set("status_id", "o", Vec::new());
        f.set_expression("status_id = 3").unwrap();
        assert_eq!(
            f.into_query(),
            pairs(&[
                ("set_filter", "1"),
                ("f[]", "status_id"),
                ("op[status_id]", "="),
                ("v[status_id][]", "3"),
            ])
        );
        assert!(Filters::default().into_query().is_empty());
    }

    #[test]
    fn user_values_are_ids_or_me() {
        assert_eq!(user_value("ME").unwrap(), "me");
        assert_eq!(user_value(" 21 ").unwrap(), "21");
        assert!(user_value("timothy").is_err());
        assert!(user_value("").is_err());
    }

    #[test]
    fn parse_kv_splits_on_first_equals() {
        assert_eq!(
            parse_kv("updated_on=>=2024-01-01").unwrap(),
            ("updated_on".into(), ">=2024-01-01".into())
        );
        assert!(parse_kv("novalue").is_err());
        assert!(parse_kv("=x").is_err());
    }
}
