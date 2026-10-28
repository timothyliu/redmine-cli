use std::io::{self, IsTerminal, Read, Write};

use serde_json::{Value, json};

use super::unwrap_key;
use crate::cli::{AuthCommand, LoginArgs};
use crate::client::Client;
use crate::context::Context;
use crate::credentials::{self, ENV_API_KEY, ENV_URL, Source, StoredProfile};
use crate::error::{AppError, Result};
use crate::output::{cell, get_path};

pub fn run(ctx: &Context, cmd: AuthCommand) -> Result<()> {
    match cmd {
        AuthCommand::Login(args) => login(ctx, args),
        AuthCommand::Logout => logout(ctx),
        AuthCommand::Status => status(ctx),
    }
}

fn login(ctx: &Context, args: LoginArgs) -> Result<()> {
    let interactive = io::stdin().is_terminal();

    let raw_url = match args.url.or_else(|| std::env::var(ENV_URL).ok()) {
        Some(u) if !u.trim().is_empty() => u,
        _ if interactive && !args.with_token => prompt("Redmine URL: ")?,
        _ => {
            return Err(AppError::Usage(format!(
                "no URL: pass --url or set {ENV_URL}"
            )));
        }
    };
    let url = credentials::normalize_url(&raw_url)?;

    let api_key = if args.with_token {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        s
    } else if interactive {
        rpassword::prompt_password(format!("API key for {url} (My account > API access key): "))?
    } else {
        return Err(AppError::Usage(
            "stdin is not a terminal: pipe the key in with --with-token".into(),
        ));
    };
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err(AppError::Usage("API key is empty".into()));
    }

    let user = if args.no_verify {
        Value::Null
    } else {
        current_user(&Client::new(&url, api_key.clone(), false)?)?
    };

    credentials::store_profile(
        &ctx.profile,
        &StoredProfile {
            url: url.clone(),
            api_key,
        },
    )?;

    let mut result = json!({
        "profile": ctx.profile,
        "url": url,
        "stored_in": "keychain",
        "verified": !args.no_verify,
        "user": user,
    });
    if std::env::var(ENV_API_KEY).is_ok_and(|v| !v.trim().is_empty()) {
        result["note"] = json!(format!(
            "{ENV_API_KEY} is set and takes precedence over the keychain"
        ));
    }
    ctx.out.object(result, |r| {
        let who = cell(get_path(r, "user.login"));
        let as_user = if who.is_empty() {
            String::new()
        } else {
            format!(" as {who}")
        };
        let mut s = format!(
            "Logged in to {}{as_user}; saved to the keychain (profile {:?})",
            cell(r.get("url")),
            ctx.profile
        );
        if let Some(note) = r.get("note") {
            s.push_str(&format!("\nnote: {}", cell(Some(note))));
        }
        s
    })
}

fn logout(ctx: &Context) -> Result<()> {
    let removed = credentials::delete_profile(&ctx.profile)?;
    ctx.out.object(
        json!({ "profile": ctx.profile, "removed": removed }),
        |_| {
            if removed {
                format!("Removed profile {:?} from the keychain", ctx.profile)
            } else {
                format!("No keychain entry for profile {:?}", ctx.profile)
            }
        },
    )
}

fn status(ctx: &Context) -> Result<()> {
    let creds = ctx.credentials()?;
    let key_source = creds.api_key.as_ref().map(|(_, s)| *s);
    let mut result = json!({
        "profile": ctx.profile,
        "url": creds.url,
        "url_source": creds.url_source,
        "api_key_source": key_source,
        "authenticated": false,
    });
    let Some((key, _)) = creds.api_key else {
        return Err(AppError::Config(format!(
            "no API key for {}: set {ENV_API_KEY} or run `redmine auth login`",
            creds.url
        )));
    };
    result["user"] = current_user(&Client::new(&creds.url, key, false)?)?;
    result["authenticated"] = json!(true);
    ctx.out.object(result, |r| {
        format!(
            "Logged in to {} as {}\n  url from:     {}\n  api key from: {}\n  profile:      {}",
            cell(r.get("url")),
            cell(get_path(r, "user.login")),
            source_label(creds.url_source),
            key_source.map_or("none", source_label),
            ctx.profile
        )
    })
}

fn source_label(s: Source) -> &'static str {
    match s {
        Source::Env => "environment",
        Source::Keychain => "keychain",
    }
}

/// Verifies the key and returns a minimal, key-free view of the user.
fn current_user(client: &Client) -> Result<Value> {
    let user = unwrap_key(client.get("/users/current.json", &[])?, "user")?;
    Ok(json!({
        "id": user.get("id"),
        "login": user.get("login"),
        "name": format!("{} {}", cell(user.get("firstname")), cell(user.get("lastname"))).trim(),
    }))
}

fn prompt(label: &str) -> Result<String> {
    eprint!("{label}");
    io::stderr().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}
