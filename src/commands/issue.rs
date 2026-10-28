use std::fmt::Write as _;

use reqwest::Method;
use serde_json::{Map, Value, json};

use super::lookup::{self, Kind};
use super::{id_value, parse_kv, read_text, unwrap_key};
use crate::cli::{
    IssueCommand, IssueCommentArgs, IssueCreateArgs, IssueFields, IssueListArgs, IssueUpdateArgs,
    IssueViewArgs,
};
use crate::client::Client;
use crate::context::Context;
use crate::error::{AppError, Result};
use crate::output::{Column, cell, get_path, pretty};

const LIST_COLUMNS: &[Column] = &[
    ("ID", "id"),
    ("TRACKER", "tracker.name"),
    ("STATUS", "status.name"),
    ("PRIORITY", "priority.name"),
    ("ASSIGNEE", "assigned_to.name"),
    ("SUBJECT", "subject"),
];

pub fn run(ctx: &Context, cmd: IssueCommand) -> Result<()> {
    let (base, client) = ctx.client()?;
    match cmd {
        IssueCommand::List(args) => list(ctx, &client, args),
        IssueCommand::View(args) => view(ctx, &client, &base, args),
        IssueCommand::Create(args) => create(ctx, &client, &base, args),
        IssueCommand::Update(args) => update(ctx, &client, &base, args),
        IssueCommand::Comment(args) => comment(ctx, &client, &base, args),
    }
}

fn list(ctx: &Context, client: &Client, args: IssueListArgs) -> Result<()> {
    let mut q: Vec<(String, String)> = Vec::new();
    if let Some(p) = args.project {
        q.push(("project_id".into(), p));
    }
    // A saved query carries its own status filter; only default to "open" without one.
    match (&args.status, args.query) {
        (Some(s), _) => q.push(("status_id".into(), lookup::status_filter(client, s)?)),
        (None, None) => q.push(("status_id".into(), "open".into())),
        (None, Some(_)) => {}
    }
    if let Some(a) = args.assignee {
        q.push(("assigned_to_id".into(), a));
    }
    if let Some(a) = args.author {
        q.push(("author_id".into(), a));
    }
    if let Some(t) = &args.tracker {
        q.push((
            "tracker_id".into(),
            lookup::resolve(client, Kind::Tracker, t)?,
        ));
    }
    if let Some(p) = &args.priority {
        q.push((
            "priority_id".into(),
            lookup::resolve(client, Kind::Priority, p)?,
        ));
    }
    if let Some(id) = args.query {
        q.push(("query_id".into(), id.to_string()));
    }
    if let Some(s) = args.sort {
        q.push(("sort".into(), s));
    }
    for f in &args.filters {
        q.push(parse_kv(f)?);
    }
    let value = client.list("/issues.json", "issues", q, &args.page)?;
    ctx.out.list(value, "issues", LIST_COLUMNS)
}

fn view(ctx: &Context, client: &Client, base: &str, args: IssueViewArgs) -> Result<()> {
    let mut q = Vec::new();
    if !args.include.is_empty() {
        q.push(("include".to_string(), args.include.join(",")));
    }
    let issue = unwrap_key(
        client.get(&format!("/issues/{}.json", args.id), &q)?,
        "issue",
    )?;
    ctx.out.object(issue, |i| render_issue(i, base))
}

fn create(ctx: &Context, client: &Client, base: &str, args: IssueCreateArgs) -> Result<()> {
    let mut m = issue_payload(client, args.fields)?;
    m.insert("project_id".into(), id_value(args.project));
    m.insert("subject".into(), args.subject.into());
    if !args.watchers.is_empty() {
        let ids = args
            .watchers
            .iter()
            .map(|w| lookup::resolve_user(client, w).map(id_value))
            .collect::<Result<Vec<_>>>()?;
        m.insert("watcher_user_ids".into(), ids.into());
    }
    let resp = client
        .send(
            Method::POST,
            "/issues.json",
            &[],
            Some(&json!({ "issue": m })),
        )?
        .ok_or_else(|| AppError::Response("POST /issues.json returned an empty body".into()))?;
    if client.dry_run {
        return ctx.out.object(resp, pretty);
    }
    let issue = unwrap_key(resp, "issue")?;
    ctx.out.object(issue, |i| render_issue(i, base))
}

fn update(ctx: &Context, client: &Client, base: &str, args: IssueUpdateArgs) -> Result<()> {
    let mut m = issue_payload(client, args.fields)?;
    if let Some(s) = args.subject {
        m.insert("subject".into(), s.into());
    }
    if let Some(n) = read_text(args.notes, args.notes_file.as_deref())? {
        m.insert("notes".into(), n.into());
        if args.private_notes {
            m.insert("private_notes".into(), true.into());
        }
    }
    if m.is_empty() {
        return Err(AppError::Usage(
            "nothing to update: pass at least one field (see `redmine issue update --help`)".into(),
        ));
    }

    let path = format!("/issues/{}.json", args.id);
    let resp = client.send(
        Method::PUT,
        &path,
        &[],
        Some(&json!({ "issue": m.clone() })),
    )?;
    if client.dry_run {
        return ctx.out.object(resp.unwrap_or(Value::Null), pretty);
    }
    let issue = unwrap_key(client.get(&path, &[])?, "issue")?;
    let ignored = ignored_changes(&m, &issue);
    ctx.out.object(issue, |i| render_issue(i, base))?;
    if ignored.is_empty() {
        return Ok(());
    }
    // Redmine answers 204 even when it silently drops a change (e.g. a status
    // transition the workflow does not allow), so surface it as a failure.
    Err(AppError::NotApplied(format!(
        "issue #{} was saved but these changes were ignored by the server: {}. \
         Check the workflow/permissions, e.g. `redmine issue view {} --include allowed_statuses`",
        args.id,
        ignored.join(", "),
        args.id
    )))
}

fn comment(ctx: &Context, client: &Client, base: &str, args: IssueCommentArgs) -> Result<()> {
    let body = read_text(args.body, args.body_file.as_deref())?
        .filter(|b| !b.trim().is_empty())
        .ok_or_else(|| AppError::Usage("comment body is empty".into()))?;
    let mut m = Map::new();
    m.insert("notes".into(), body.into());
    if args.private {
        m.insert("private_notes".into(), true.into());
    }
    let path = format!("/issues/{}.json", args.id);
    let resp = client.send(Method::PUT, &path, &[], Some(&json!({ "issue": m })))?;
    if client.dry_run {
        return ctx.out.object(resp.unwrap_or(Value::Null), pretty);
    }
    let issue = unwrap_key(
        client.get(&path, &[("include".into(), "journals".into())])?,
        "issue",
    )?;
    let journal = issue
        .get("journals")
        .and_then(Value::as_array)
        .and_then(|j| j.last())
        .cloned()
        .unwrap_or(Value::Null);
    let id = args.id;
    ctx.out
        .object(json!({ "issue_id": id, "journal": journal }), |v| {
            format!(
                "Commented on #{id}: {base}/issues/{id}#change-{}",
                cell(get_path(v, "journal.id"))
            )
        })
}

fn issue_payload(client: &Client, f: IssueFields) -> Result<Map<String, Value>> {
    let mut m = Map::new();
    if let Some(d) = read_text(f.description, f.description_file.as_deref())? {
        m.insert("description".into(), d.into());
    }
    if let Some(t) = f.tracker {
        m.insert(
            "tracker_id".into(),
            id_value(lookup::resolve(client, Kind::Tracker, &t)?),
        );
    }
    if let Some(s) = f.status {
        m.insert(
            "status_id".into(),
            id_value(lookup::resolve(client, Kind::Status, &s)?),
        );
    }
    if let Some(p) = f.priority {
        m.insert(
            "priority_id".into(),
            id_value(lookup::resolve(client, Kind::Priority, &p)?),
        );
    }
    if let Some(a) = f.assignee {
        let v = if a.trim().eq_ignore_ascii_case("none") {
            Value::String(String::new())
        } else {
            id_value(lookup::resolve_user(client, &a)?)
        };
        m.insert("assigned_to_id".into(), v);
    }
    if let Some(p) = f.parent {
        let v = match p.trim() {
            s if s.eq_ignore_ascii_case("none") => Value::String(String::new()),
            s => s.parse::<u64>().map(Value::from).map_err(|_| {
                AppError::Usage(format!(
                    "--parent must be an issue id or \"none\", got {p:?}"
                ))
            })?,
        };
        m.insert("parent_issue_id".into(), v);
    }
    if let Some(d) = f.start_date {
        m.insert("start_date".into(), d.into());
    }
    if let Some(d) = f.due_date {
        m.insert("due_date".into(), d.into());
    }
    if let Some(h) = f.estimated_hours {
        m.insert("estimated_hours".into(), h.into());
    }
    if let Some(r) = f.done_ratio {
        m.insert("done_ratio".into(), r.into());
    }
    if !f.custom_fields.is_empty() {
        let cfs = f
            .custom_fields
            .iter()
            .map(|kv| {
                let (id, value) = parse_kv(kv)?;
                let id: u64 = id.parse().map_err(|_| {
                    AppError::Usage(format!("--custom-field id must be numeric, got {id:?}"))
                })?;
                Ok(json!({ "id": id, "value": value }))
            })
            .collect::<Result<Vec<_>>>()?;
        m.insert("custom_fields".into(), cfs.into());
    }
    Ok(m)
}

/// Requested id fields whose value did not stick after the update.
fn ignored_changes(sent: &Map<String, Value>, issue: &Value) -> Vec<String> {
    [
        ("status_id", "status"),
        ("tracker_id", "tracker"),
        ("priority_id", "priority"),
        ("assigned_to_id", "assigned_to"),
    ]
    .iter()
    .filter_map(|(field, obj)| {
        let want = sent.get(*field)?.as_u64()?;
        let got = issue
            .get(*obj)
            .and_then(|o| o.get("id"))
            .and_then(Value::as_u64);
        (got != Some(want)).then(|| {
            format!(
                "{field}={want} (still {})",
                got.map_or_else(|| "unset".to_string(), |g| g.to_string())
            )
        })
    })
    .collect()
}

fn render_issue(i: &Value, base: &str) -> String {
    let g = |p: &str| cell(get_path(i, p));
    let mut s = String::new();
    let _ = writeln!(s, "#{} {}", g("id"), g("subject"));
    let _ = writeln!(s, "{base}/issues/{}\n", g("id"));
    for (label, path) in [
        ("Project", "project"),
        ("Tracker", "tracker"),
        ("Status", "status"),
        ("Priority", "priority"),
        ("Author", "author"),
        ("Assignee", "assigned_to"),
        ("Parent", "parent.id"),
        ("Start", "start_date"),
        ("Due", "due_date"),
        ("Done", "done_ratio"),
        ("Estimated", "estimated_hours"),
        ("Created", "created_on"),
        ("Updated", "updated_on"),
    ] {
        let v = g(path);
        if !v.is_empty() {
            let _ = writeln!(s, "{label:<10} {v}");
        }
    }
    for cf in array(i, "custom_fields") {
        let v = cell(cf.get("value"));
        if !v.is_empty() {
            let _ = writeln!(s, "{:<10} {v}", cell(cf.get("name")));
        }
    }

    let desc = i
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if !desc.is_empty() {
        let _ = writeln!(s, "\n{desc}");
    }

    let children = array(i, "children");
    if !children.is_empty() {
        let _ = writeln!(s, "\nSubtasks:");
        for c in children {
            let _ = writeln!(s, "  #{} {}", cell(c.get("id")), cell(c.get("subject")));
        }
    }
    let relations = array(i, "relations");
    if !relations.is_empty() {
        let _ = writeln!(s, "\nRelations:");
        for r in relations {
            let _ = writeln!(
                s,
                "  #{} {} #{}",
                cell(r.get("issue_id")),
                cell(r.get("relation_type")),
                cell(r.get("issue_to_id"))
            );
        }
    }
    let attachments = array(i, "attachments");
    if !attachments.is_empty() {
        let _ = writeln!(s, "\nAttachments:");
        for a in attachments {
            let _ = writeln!(
                s,
                "  {} {}",
                cell(a.get("filename")),
                cell(a.get("content_url"))
            );
        }
    }
    let journals = array(i, "journals");
    if !journals.is_empty() {
        let _ = writeln!(s, "\nHistory:");
        for j in journals {
            let _ = writeln!(
                s,
                "\n[{}] {} (#change-{})",
                cell(j.get("created_on")),
                cell(j.get("user")),
                cell(j.get("id"))
            );
            for d in j
                .get("details")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let _ = writeln!(
                    s,
                    "  - {}: {} -> {}",
                    cell(d.get("name")),
                    cell(d.get("old_value")),
                    cell(d.get("new_value"))
                );
            }
            let notes = j.get("notes").and_then(Value::as_str).unwrap_or("").trim();
            if !notes.is_empty() {
                let _ = writeln!(s, "{notes}");
            }
        }
    }
    s
}

fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_changes_reports_status_that_did_not_stick() {
        let sent = json!({"status_id": 5, "notes": "x"});
        let issue = json!({"status": {"id": 1, "name": "New"}});
        let ignored = ignored_changes(sent.as_object().unwrap(), &issue);
        assert_eq!(ignored, vec!["status_id=5 (still 1)"]);
    }

    #[test]
    fn ignored_changes_accepts_applied_and_unassign() {
        let sent = json!({"status_id": 5, "assigned_to_id": ""});
        let issue = json!({"status": {"id": 5}});
        assert!(ignored_changes(sent.as_object().unwrap(), &issue).is_empty());
    }
}
