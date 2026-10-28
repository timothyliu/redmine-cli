use std::fmt::Write as _;

use super::unwrap_key;
use crate::cli::ProjectCommand;
use crate::context::Context;
use crate::error::{AppError, Result};
use crate::output::{Column, cell, get_path};

const LIST_COLUMNS: &[Column] = &[
    ("ID", "id"),
    ("IDENTIFIER", "identifier"),
    ("NAME", "name"),
    ("PARENT", "parent.name"),
];

pub fn run(ctx: &Context, cmd: ProjectCommand) -> Result<()> {
    let (base, client) = ctx.client()?;
    match cmd {
        ProjectCommand::List(page) => {
            let value = client.list("/projects.json", "projects", Vec::new(), &page)?;
            ctx.out.list(value, "projects", LIST_COLUMNS)
        }
        ProjectCommand::View { id, include } => {
            // The id becomes a path segment; refuse anything that could reach another endpoint.
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(AppError::Usage(format!(
                    "project must be a numeric id or identifier ([A-Za-z0-9_-]), got {id:?}"
                )));
            }
            let mut q = Vec::new();
            if !include.is_empty() {
                q.push(("include".to_string(), include.join(",")));
            }
            let project = unwrap_key(client.get(&format!("/projects/{id}.json"), &q)?, "project")?;
            ctx.out.object(project, |p| {
                let g = |path: &str| cell(get_path(p, path));
                let mut s = String::new();
                let _ = writeln!(s, "{} ({})", g("name"), g("identifier"));
                let _ = writeln!(s, "{base}/projects/{}\n", g("identifier"));
                for (label, path) in [
                    ("ID", "id"),
                    ("Parent", "parent"),
                    ("Public", "is_public"),
                    ("Homepage", "homepage"),
                    ("Trackers", "trackers"),
                    ("Modules", "enabled_modules"),
                    ("Created", "created_on"),
                    ("Updated", "updated_on"),
                ] {
                    let v = g(path);
                    if !v.is_empty() {
                        let _ = writeln!(s, "{label:<10} {v}");
                    }
                }
                let desc = g("description");
                if !desc.trim().is_empty() {
                    let _ = writeln!(s, "\n{}", p["description"].as_str().unwrap_or("").trim());
                }
                s
            })
        }
    }
}
