use reqwest::Method;
use serde_json::{Map, json};

use super::lookup::{self, Kind};
use super::{Filters, id_value, unwrap_key, user_value};
use crate::cli::{TimeCommand, TimeListArgs, TimeLogArgs};
use crate::client::Client;
use crate::context::Context;
use crate::error::{AppError, Result};
use crate::output::{Column, cell, get_path, pretty};

const LIST_COLUMNS: &[Column] = &[
    ("ID", "id"),
    ("DATE", "spent_on"),
    ("HOURS", "hours"),
    ("USER", "user.name"),
    ("ACTIVITY", "activity.name"),
    ("ISSUE", "issue.id"),
    ("COMMENT", "comments"),
];

pub fn run(ctx: &Context, cmd: TimeCommand) -> Result<()> {
    let (_, client) = ctx.client()?;
    match cmd {
        TimeCommand::List(args) => list(ctx, &client, args),
        TimeCommand::Log(args) => log(ctx, &client, args),
    }
}

fn list(ctx: &Context, client: &Client, args: TimeListArgs) -> Result<()> {
    let mut filters = Filters::default();
    if let Some(u) = &args.user {
        filters.set("user_id", "=", vec![user_value(u)?]);
    }
    match (args.from, args.to) {
        (Some(from), Some(to)) => filters.set("spent_on", "><", vec![from, to]),
        (Some(from), None) => filters.set("spent_on", ">=", vec![from]),
        (None, Some(to)) => filters.set("spent_on", "<=", vec![to]),
        (None, None) => {}
    }
    // project_id and issue_id scope the request rather than filter it.
    let mut q = filters.into_query();
    if let Some(p) = args.project {
        q.push(("project_id".into(), p));
    }
    if let Some(i) = args.issue {
        q.push(("issue_id".into(), i.to_string()));
    }
    let value = client.list("/time_entries.json", "time_entries", q, &args.page)?;
    ctx.out.list(value, "time_entries", LIST_COLUMNS)
}

fn log(ctx: &Context, client: &Client, args: TimeLogArgs) -> Result<()> {
    let mut m = Map::new();
    if let Some(i) = args.issue {
        m.insert("issue_id".into(), i.into());
    }
    if let Some(p) = args.project {
        m.insert("project_id".into(), id_value(p));
    }
    m.insert("hours".into(), args.hours.into());
    if let Some(a) = args.activity {
        m.insert(
            "activity_id".into(),
            id_value(lookup::resolve(client, Kind::Activity, &a)?),
        );
    }
    if let Some(c) = args.comment {
        m.insert("comments".into(), c.into());
    }
    if let Some(d) = args.date {
        m.insert("spent_on".into(), d.into());
    }
    let resp = client
        .send(
            Method::POST,
            "/time_entries.json",
            &[],
            Some(&json!({ "time_entry": m })),
        )?
        .ok_or_else(|| {
            AppError::Response("POST /time_entries.json returned an empty body".into())
        })?;
    if client.dry_run {
        return ctx.out.object(resp, pretty);
    }
    let entry = unwrap_key(resp, "time_entry")?;
    ctx.out.object(entry, |e| {
        let g = |p: &str| cell(get_path(e, p));
        format!(
            "Logged {}h on {} ({}) — time entry #{}",
            g("hours"),
            g("spent_on"),
            g("activity.name"),
            g("id")
        )
    })
}
