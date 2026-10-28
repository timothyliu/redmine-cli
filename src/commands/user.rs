use std::fmt::Write as _;

use serde_json::Value;

use super::{redact, unwrap_key};
use crate::cli::UserCommand;
use crate::context::Context;
use crate::error::Result;
use crate::output::{cell, get_path};

pub fn run(ctx: &Context, cmd: UserCommand) -> Result<()> {
    let (_, client) = ctx.client()?;
    let path = match cmd {
        UserCommand::Me => "/users/current.json".to_string(),
        UserCommand::View { id } => format!("/users/{id}.json"),
    };
    let mut user = unwrap_key(client.get(&path, &[])?, "user")?;
    redact(&mut user);
    ctx.out.object(user, render_user)
}

pub fn render_user(u: &Value) -> String {
    let g = |p: &str| cell(get_path(u, p));
    let mut s = String::new();
    let _ = writeln!(s, "{} {} ({})", g("firstname"), g("lastname"), g("login"));
    for (label, path) in [
        ("ID", "id"),
        ("Email", "mail"),
        ("Admin", "admin"),
        ("Created", "created_on"),
        ("Last login", "last_login_on"),
    ] {
        let v = g(path);
        if !v.is_empty() {
            let _ = writeln!(s, "{label:<11} {v}");
        }
    }
    s
}
