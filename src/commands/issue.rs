use std::collections::BTreeMap;
use std::fmt::Write as _;

use reqwest::Method;
use serde_json::{Map, Value, json};

use super::lookup::{self, Kind};
use super::{Filters, id_value, parse_kv, project_segment, read_text, unwrap_key, user_value};
use crate::cli::{
    IssueCommand, IssueCommentArgs, IssueCreateArgs, IssueFields, IssueListArgs, IssueSearchArgs,
    IssueUpdateArgs, IssueViewArgs, PageArgs,
};
use crate::client::Client;
use crate::context::Context;
use crate::error::{AppError, Result};
use crate::output::{Column, Format, cell, get_path, pretty};

const LIST_COLUMNS: &[Column] = &[
    ("ID", "id"),
    ("TRACKER", "tracker.name"),
    ("STATUS", "status.name"),
    ("PRIORITY", "priority.name"),
    ("ASSIGNEE", "assigned_to.name"),
    ("SUBJECT", "subject"),
];

const SEARCH_COLUMNS: &[Column] = &[("ID", "id"), ("DATE", "datetime"), ("TITLE", "title")];

pub fn run(ctx: &Context, cmd: IssueCommand) -> Result<()> {
    let (base, client) = ctx.client()?;
    match cmd {
        IssueCommand::List(args) => list(ctx, &client, args),
        IssueCommand::Search(args) => search(ctx, &client, &base, args),
        IssueCommand::View(args) => view(ctx, &client, &base, args),
        IssueCommand::Create(args) => create(ctx, &client, &base, args),
        IssueCommand::Update(args) => update(ctx, &client, &base, args),
        IssueCommand::Comment(args) => comment(ctx, &client, &base, args),
    }
}

/// Full-text search via /search.json (Redmine 3.3+; also covers comments unless
/// --titles-only). It answers 404 on older servers (and for an unknown project), so
/// fall back to "contains" filters on subject and description, shaped like search
/// results.
fn search(ctx: &Context, client: &Client, base: &str, args: IssueSearchArgs) -> Result<()> {
    let project = args.project.as_deref().map(project_segment).transpose()?;
    let path = match project {
        Some(p) => format!("/projects/{p}/search.json"),
        None => "/search.json".to_string(),
    };
    // Redmine 3.3.0-3.4.9 and 4.0.0-4.0.2 jump to the issue for a bare number ("123",
    // "#123") instead of searching; a quoted phrase searches the same token everywhere.
    let digits = args.text.trim().trim_start_matches('#');
    let text = if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        format!("\"{}\"", args.text.trim())
    } else {
        args.text.clone()
    };
    let mut q = vec![
        ("q".to_string(), text),
        ("issues".to_string(), "1".to_string()),
    ];
    if args.open {
        q.push(("open_issues".into(), "1".into()));
    }
    if args.titles_only {
        q.push(("titles_only".into(), "1".into()));
    }
    let fulltext = match client.list(&path, "results", q, &args.page) {
        Ok(v) => Some(v),
        Err(AppError::Api { status: 404, .. }) => None,
        Err(e) => return Err(e),
    };
    let value = match fulltext {
        Some(mut v) => {
            v["search"] = json!("fulltext");
            v
        }
        None => {
            let v = search_by_filters(client, base, project, &args)?;
            if ctx.out.format == Format::Text {
                eprintln!("note: searched subject and description only, not comments");
            }
            v
        }
    };
    ctx.out.list(value, "results", SEARCH_COLUMNS)
}

fn search_by_filters(
    client: &Client,
    base: &str,
    project: Option<&str>,
    args: &IssueSearchArgs,
) -> Result<Value> {
    let fields: &[&str] = if args.titles_only {
        &["subject"]
    } else {
        &["subject", "description"]
    };
    let every = PageArgs {
        limit: 100,
        offset: 0,
        all: true,
    };
    // Filters are ANDed, so each field is a separate query; merge by id.
    let mut found: BTreeMap<u64, Value> = BTreeMap::new();
    let mut truncated = false;
    for field in fields {
        let mut filters = Filters::default();
        filters.set("status_id", if args.open { "o" } else { "*" }, Vec::new());
        filters.set(field, "~", vec![args.text.clone()]);
        let mut q = filters.into_query();
        if let Some(p) = project {
            q.push(("project_id".into(), p.to_string()));
        }
        let page = client.list("/issues.json", "issues", q, &every)?;
        truncated |= page.get("truncated") == Some(&json!(true));
        for issue in page["issues"].as_array().into_iter().flatten() {
            if let Some(id) = issue["id"].as_u64() {
                found
                    .entry(id)
                    .or_insert_with(|| search_result(base, issue));
            }
        }
    }

    let total = found.len();
    let take = if args.page.all {
        usize::MAX
    } else {
        args.page.limit.min(100) as usize
    };
    let results: Vec<Value> = found
        .into_values()
        .rev()
        .skip(args.page.offset as usize)
        .take(take)
        .collect();
    let limit = if args.page.all { results.len() } else { take };
    let mut value = json!({
        "results": results,
        "total_count": total,
        "offset": args.page.offset,
        "limit": limit,
        "search": "subject_description",
    });
    if truncated {
        value["truncated"] = json!(true);
    }
    Ok(value)
}

/// Shapes an issue like a /search.json result.
fn search_result(base: &str, issue: &Value) -> Value {
    let g = |path: &str| cell(get_path(issue, path));
    json!({
        "id": issue["id"],
        "title": format!("{} #{} ({}): {}", g("tracker.name"), g("id"), g("status.name"), g("subject")),
        "type": "issue",
        "url": format!("{base}/issues/{}", g("id")),
        "description": issue["description"],
        "datetime": issue["created_on"],
    })
}

fn list(ctx: &Context, client: &Client, args: IssueListArgs) -> Result<()> {
    // Redmine applies a saved query's own filters and ignores any others.
    let filtered = args.status.is_some()
        || args.assignee.is_some()
        || args.author.is_some()
        || args.tracker.is_some()
        || args.priority.is_some()
        || !args.filters.is_empty();
    if args.query.is_some() && filtered {
        return Err(AppError::Usage(
            "--query applies the saved query's filters; Redmine ignores other filters with it"
                .into(),
        ));
    }

    let mut filters = Filters::default();
    if let Some(s) = &args.status {
        let (op, values) = lookup::status_filter(client, s)?;
        filters.set("status_id", op, values);
    }
    if let Some(a) = &args.assignee {
        filters.set("assigned_to_id", "=", vec![user_value(a)?]);
    }
    if let Some(a) = &args.author {
        filters.set("author_id", "=", vec![user_value(a)?]);
    }
    if let Some(t) = &args.tracker {
        let id = lookup::resolve(client, Kind::Tracker, t)?;
        filters.set("tracker_id", "=", vec![id]);
    }
    if let Some(p) = &args.priority {
        let id = lookup::resolve(client, Kind::Priority, p)?;
        filters.set("priority_id", "=", vec![id]);
    }
    for f in &args.filters {
        filters.set_expression(f)?;
    }
    if args.query.is_none() && !filters.has("status_id") {
        filters.set("status_id", "o", Vec::new());
    }

    let mut q = filters.into_query();
    if let Some(p) = args.project {
        q.push(("project_id".into(), p));
    }
    if let Some(id) = args.query {
        q.push(("query_id".into(), id.to_string()));
    }
    if let Some(s) = args.sort {
        q.push(("sort".into(), s));
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
