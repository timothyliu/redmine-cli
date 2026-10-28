use clap::{Arg, ArgAction, CommandFactory};
use serde_json::{Value, json};

use crate::cli::{Cli, SchemaArgs};
use crate::context::Context;
use crate::credentials::{ENV_API_KEY, ENV_NO_KEYCHAIN, ENV_URL};
use crate::error::{AppError, EXIT_CODES, Result};
use crate::output::pretty;

pub fn run(ctx: &Context, args: SchemaArgs) -> Result<()> {
    let root = Cli::command();
    let mut cmd = &root;
    for name in &args.command {
        cmd = cmd.find_subcommand(name).ok_or_else(|| {
            AppError::Usage(format!("unknown command {:?}", args.command.join(" ")))
        })?;
    }

    let mut value = describe(cmd);
    if args.command.is_empty() {
        let exit_codes: serde_json::Map<String, Value> = EXIT_CODES
            .iter()
            .map(|(code, meaning)| (code.to_string(), json!(meaning)))
            .collect();
        value["exit_codes"] = exit_codes.into();
        value["environment"] = json!({
            ENV_URL: "Redmine base URL (overrides the keychain)",
            ENV_API_KEY: "API key; requires REDMINE_URL (never paired with a keychain URL)",
            "REDMINE_PROFILE": "keychain profile name (default: \"default\")",
            "REDMINE_FORMAT": "json | text",
            "REDMINE_TIMEOUT": "HTTP timeout in seconds (default: 30)",
            "REDMINE_MAX_ITEMS": "maximum items collected by --all (default: 10000)",
            "REDMINE_MAX_BYTES": "maximum estimated memory, in bytes, of items collected by --all (default: 268435456)",
            ENV_NO_KEYCHAIN: "set to 1 to never read or write the keychain",
        });
        value["conventions"] = json!({
            "stdout": "result only; JSON when not a terminal or with --format json",
            "stderr": "errors as {\"error\": {\"code\", \"message\", ...}} in JSON mode",
            "lists": "{\"<collection>\": [...], \"total_count\", \"offset\", \"limit\"}",
            "objects": "unwrapped (no {\"issue\": ...} envelope)",
            "secrets": "the exact API key string (not other encodings of it) and api_key fields are masked in responses and errors",
            "truncation": "--all results carry \"truncated\": true when REDMINE_MAX_ITEMS or REDMINE_MAX_BYTES was hit",
        });
    }
    ctx.out.object(value, pretty)
}

fn describe(cmd: &clap::Command) -> Value {
    let args: Vec<Value> = cmd
        .get_arguments()
        .filter(|a| !matches!(a.get_id().as_str(), "help" | "version"))
        .map(describe_arg)
        .collect();
    let subcommands: Vec<Value> = cmd
        .get_subcommands()
        .filter(|c| c.get_name() != "help")
        .map(describe)
        .collect();
    let mut v = json!({
        "name": cmd.get_name(),
        "about": cmd.get_about().map(|s| s.to_string()),
    });
    if !args.is_empty() {
        v["args"] = args.into();
    }
    if !subcommands.is_empty() {
        v["subcommands"] = subcommands.into();
    }
    v
}

fn describe_arg(a: &Arg) -> Value {
    let mut v = json!({
        "name": a.get_id().as_str(),
        "help": a.get_help().map(|s| s.to_string()),
        "required": a.is_required_set(),
    });
    if a.is_positional() {
        v["positional"] = true.into();
    }
    if let Some(long) = a.get_long() {
        v["long"] = format!("--{long}").into();
    }
    if let Some(short) = a.get_short() {
        v["short"] = format!("-{short}").into();
    }
    let takes_value = a.get_action().takes_values();
    v["takes_value"] = takes_value.into();
    if matches!(a.get_action(), ArgAction::Append) || a.get_value_delimiter().is_some() {
        v["multiple"] = true.into();
    }
    if a.is_global_set() {
        v["global"] = true.into();
    }
    let possible: Vec<String> = a
        .get_possible_values()
        .iter()
        .map(|p| p.get_name().to_string())
        .collect();
    if takes_value && !possible.is_empty() {
        v["possible_values"] = possible.into();
    }
    let defaults: Vec<String> = a
        .get_default_values()
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    if !defaults.is_empty() {
        v["default"] = defaults.join(",").into();
    }
    if let Some(env) = a.get_env() {
        v["env"] = env.to_string_lossy().into_owned().into();
    }
    v
}
