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

/// Numeric ids become JSON numbers; identifiers stay strings.
pub(crate) fn id_value(s: String) -> Value {
    s.parse::<u64>()
        .map(Value::from)
        .unwrap_or(Value::String(s))
}

#[cfg(test)]
mod tests {
    use super::*;

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
