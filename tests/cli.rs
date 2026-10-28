//! End-to-end tests against a mock Redmine server. The keychain is disabled
//! (REDMINE_NO_KEYCHAIN) so these never touch the developer's real credentials.

use assert_cmd::Command;
use mockito::{Matcher, Server, ServerGuard};
use predicates::prelude::*;
use serde_json::{Value, json};

const KEY: &str = "test-key-123";

fn redmine(server: &ServerGuard) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("redmine"));
    cmd.env_clear()
        .env("REDMINE_URL", server.url())
        .env("REDMINE_API_KEY", KEY)
        .env("REDMINE_NO_KEYCHAIN", "1");
    cmd
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

fn stderr_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stderr).expect("stderr is JSON")
}

#[test]
fn issue_list_defaults_to_open_and_sends_api_key_header() {
    let mut server = Server::new();
    let mock = server
        .mock("GET", "/issues.json")
        .match_header("x-redmine-api-key", KEY)
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("set_filter".into(), "1".into()),
            Matcher::UrlEncoded("f[]".into(), "status_id".into()),
            Matcher::UrlEncoded("op[status_id]".into(), "o".into()),
            Matcher::UrlEncoded("project_id".into(), "web".into()),
            Matcher::UrlEncoded("limit".into(), "25".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body(r#"{"issues":[{"id":1,"subject":"A","status":{"id":1,"name":"New"}}],"total_count":1,"offset":0,"limit":25}"#)
        .create();

    let out = redmine(&server)
        .args(["issue", "list", "-p", "web"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout_json(&out)["issues"][0]["subject"], "A");
    mock.assert();
}

#[test]
fn fields_projection_keeps_only_requested_paths() {
    let mut server = Server::new();
    server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .with_body(r#"{"issues":[{"id":1,"subject":"A","status":{"id":1,"name":"New"},"description":"long"}],"total_count":1}"#)
        .create();

    let out = redmine(&server)
        .args(["issue", "list", "--fields", "id,status.name"])
        .output()
        .unwrap();
    assert_eq!(
        stdout_json(&out),
        json!({"issues":[{"id":1,"status":{"name":"New"}}],"total_count":1})
    );
}

#[test]
fn not_found_is_exit_5_with_json_error() {
    let mut server = Server::new();
    server
        .mock("GET", "/issues/999.json")
        .with_status(404)
        .create();

    let out = redmine(&server)
        .args(["issue", "view", "999"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert!(out.stdout.is_empty());
    assert_eq!(stderr_json(&out)["error"]["code"], "not_found");
}

#[test]
fn validation_error_is_exit_6_with_details() {
    let mut server = Server::new();
    server
        .mock("POST", "/issues.json")
        .with_status(422)
        .with_body(r#"{"errors":["Subject cannot be blank"]}"#)
        .create();

    let out = redmine(&server)
        .args(["issue", "create", "-p", "web", "--subject", " "])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    let err = stderr_json(&out);
    assert_eq!(err["error"]["code"], "validation");
    assert_eq!(err["error"]["details"][0], "Subject cannot be blank");
}

#[test]
fn unauthorized_is_exit_4() {
    let mut server = Server::new();
    server
        .mock("GET", "/users/current.json")
        .with_status(401)
        .create();

    let out = redmine(&server).args(["user", "me"]).output().unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(stderr_json(&out)["error"]["code"], "unauthorized");
}

#[test]
fn missing_credentials_is_exit_3() {
    let out = Command::new(assert_cmd::cargo::cargo_bin!("redmine"))
        .env_clear()
        .env("REDMINE_NO_KEYCHAIN", "1")
        .args(["issue", "list"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let err = stderr_json(&out);
    assert_eq!(err["error"]["code"], "config");
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("REDMINE_URL")
    );
}

#[test]
fn url_without_key_is_exit_3() {
    let server = Server::new();
    let out = redmine(&server)
        .env_remove("REDMINE_API_KEY")
        .args(["issue", "list"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr_json(&out)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("REDMINE_API_KEY")
    );
}

#[test]
fn update_resolves_status_name_and_returns_issue() {
    let mut server = Server::new();
    server
        .mock("GET", "/issue_statuses.json")
        .with_body(r#"{"issue_statuses":[{"id":1,"name":"New"},{"id":5,"name":"Closed"}]}"#)
        .create();
    let put = server
        .mock("PUT", "/issues/7.json")
        .match_body(Matcher::Json(
            json!({"issue":{"status_id":5,"notes":"done"}}),
        ))
        .with_status(204)
        .create();
    server
        .mock("GET", "/issues/7.json")
        .with_body(r#"{"issue":{"id":7,"subject":"X","status":{"id":5,"name":"Closed"}}}"#)
        .create();

    let out = redmine(&server)
        .args([
            "issue", "update", "7", "--status", "closed", "--notes", "done",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout_json(&out)["status"]["name"], "Closed");
    put.assert();
}

#[test]
fn update_silently_ignored_by_server_is_reported() {
    let mut server = Server::new();
    server
        .mock("PUT", "/issues/7.json")
        .with_status(204)
        .create();
    server
        .mock("GET", "/issues/7.json")
        .with_body(r#"{"issue":{"id":7,"status":{"id":1,"name":"New"}}}"#)
        .create();

    let out = redmine(&server)
        .args(["issue", "update", "7", "--status", "5"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert_eq!(stdout_json(&out)["id"], 7);
    assert_eq!(stderr_json(&out)["error"]["code"], "not_applied");
}

#[test]
fn unknown_status_name_lists_available_names() {
    let mut server = Server::new();
    server
        .mock("GET", "/issue_statuses.json")
        .with_body(r#"{"issue_statuses":[{"id":1,"name":"New"}]}"#)
        .create();

    let out = redmine(&server)
        .args(["issue", "update", "7", "--status", "Bogus"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr_json(&out)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("available: New")
    );
}

#[test]
fn dry_run_does_not_send_writes() {
    let mut server = Server::new();
    let put = server.mock("PUT", "/issues/7.json").expect(0).create();

    let out = redmine(&server)
        .args(["--dry-run", "issue", "comment", "7", "-b", "hello"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = stdout_json(&out);
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["method"], "PUT");
    assert_eq!(v["body"], json!({"issue":{"notes":"hello"}}));
    put.assert();
}

#[test]
fn api_key_is_never_printed() {
    let mut server = Server::new();
    server
        .mock("GET", "/users/current.json")
        .with_body(r#"{"user":{"id":3,"login":"tim","api_key":"leaked"}}"#)
        .expect(2)
        .create();

    for args in [vec!["user", "me"], vec!["api", "/users/current.json"]] {
        redmine(&server)
            .args(&args)
            .assert()
            .success()
            .stdout(predicate::str::contains("leaked").not())
            .stdout(predicate::str::contains("\"login\":\"tim\""));
    }
}

#[test]
fn all_follows_pagination() {
    let mut server = Server::new();
    let mut page = |offset: &str, body: &str| {
        server
            .mock("GET", "/projects.json")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("limit".into(), "100".into()),
                Matcher::UrlEncoded("offset".into(), offset.into()),
            ]))
            .with_body(body)
            .create()
    };
    let items: Vec<Value> = (1..=100).map(|i| json!({"id": i})).collect();
    let first = page(
        "0",
        &json!({"projects": items, "total_count": 101}).to_string(),
    );
    let second = page("100", r#"{"projects":[{"id":101}],"total_count":101}"#);

    let out = redmine(&server)
        .args(["project", "list", "--all"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = stdout_json(&out);
    assert_eq!(v["projects"].as_array().unwrap().len(), 101);
    assert_eq!(v["total_count"], 101);
    first.assert();
    second.assert();
}

#[test]
fn schema_needs_no_credentials() {
    let out = Command::new(assert_cmd::cargo::cargo_bin!("redmine"))
        .env_clear()
        .env("REDMINE_NO_KEYCHAIN", "1")
        .args(["schema", "issue", "create"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = stdout_json(&out);
    assert_eq!(v["name"], "create");
    let names: Vec<&str> = v["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"subject") && names.contains(&"project"));
}

#[test]
fn text_format_renders_table() {
    let mut server = Server::new();
    server
        .mock("GET", "/enumerations/issue_priorities.json")
        .with_body(r#"{"issue_priorities":[{"id":2,"name":"Normal","is_default":true}]}"#)
        .create();

    redmine(&server)
        .args(["-o", "text", "meta", "priorities"])
        .assert()
        .success()
        .stdout("ID  NAME    DEFAULT\n2   Normal  true\n");
}

// ---- security regressions -------------------------------------------------

#[test]
fn cross_origin_redirect_is_not_followed() {
    let mut server = Server::new();
    let mut other = Server::new();
    let stolen = other.mock("GET", "/stolen").expect(0).create();
    server
        .mock("GET", "/issues/1.json")
        .with_status(302)
        .with_header("location", &format!("{}/stolen", other.url()))
        .create();

    let out = redmine(&server)
        .args(["issue", "view", "1"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(stderr_json(&out)["error"]["code"], "redirect");
    stolen.assert();
}

#[test]
fn same_origin_redirect_is_followed() {
    let mut server = Server::new();
    server
        .mock("GET", "/old.json")
        .with_status(301)
        .with_header("location", "/new.json")
        .create();
    let new = server
        .mock("GET", "/new.json")
        .match_header("x-redmine-api-key", KEY)
        .with_body(r#"{"ok":1}"#)
        .create();

    let out = redmine(&server)
        .args(["api", "/old.json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout_json(&out)["ok"], 1);
    new.assert();
}

#[test]
fn api_key_value_is_scrubbed_from_any_response() {
    let mut server = Server::new();
    server
        .mock("GET", "/users/current.xml")
        .with_header("content-type", "application/xml")
        .with_body(format!(
            "<user><login>tim</login><api_key>{KEY}</api_key></user>"
        ))
        .create();
    server
        .mock("POST", "/issues.json")
        .with_status(422)
        .with_body(format!(r#"{{"errors":["bad key {KEY}"]}}"#))
        .create();

    redmine(&server)
        .args(["api", "/users/current.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains(KEY).not())
        .stdout(predicate::str::contains("<login>tim</login>"));
    redmine(&server)
        .args(["issue", "create", "-p", "x", "--subject", "s"])
        .assert()
        .code(6)
        .stderr(predicate::str::contains(KEY).not());
}

#[test]
fn text_mode_strips_terminal_escapes() {
    let mut server = Server::new();
    let issue = json!({"issue": {
        "id": 1,
        "subject": "a\u{1b}]0;PWNED\u{7}b\u{9b}31m",
        "description": "line1\r\nline2\u{1b}[2K\u{202e}x",
    }});
    server
        .mock("GET", "/issues/1.json")
        .with_body(issue.to_string())
        .create();
    server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .with_body(json!({"issues": [issue["issue"]], "total_count": 1}).to_string())
        .create();

    for args in [
        vec!["-o", "text", "issue", "view", "1"],
        vec!["-o", "text", "issue", "list"],
    ] {
        let out = redmine(&server).args(&args).output().unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(
            !stdout
                .chars()
                .any(|c| (c.is_control() && c != '\n') || ('\u{202a}'..='\u{202e}').contains(&c)),
            "control characters leaked: {stdout:?}"
        );
        assert!(stdout.contains("a]0;PWNEDb"), "{stdout:?}");
    }
}

#[test]
fn json_mode_escapes_c1_and_bidi_controls() {
    let mut server = Server::new();
    server
        .mock("GET", "/issues/1.json")
        .with_body(json!({"issue": {"id": 1, "subject": "x\u{9b}31m\u{202e}y"}}).to_string())
        .create();

    let out = redmine(&server)
        .args(["issue", "view", "1"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    assert!(
        !stdout.contains('\u{9b}') && !stdout.contains('\u{202e}'),
        "{stdout:?}"
    );
    assert_eq!(stdout_json(&out)["subject"], "x\u{9b}31m\u{202e}y");
}

#[test]
fn all_stops_at_item_cap_against_lying_server() {
    let mut server = Server::new();
    let items: Vec<Value> = (0..100).map(|i| json!({"id": i})).collect();
    let pages = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .with_body(json!({"issues": items, "total_count": 1_000_000_000_000u64}).to_string())
        .expect(3)
        .create();

    let out = redmine(&server)
        .env("REDMINE_MAX_ITEMS", "300")
        .args(["issue", "list", "--all"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = stdout_json(&out);
    assert_eq!(v["issues"].as_array().unwrap().len(), 300);
    assert_eq!(v["truncated"], true);
    pages.assert();
}

#[test]
fn project_view_rejects_path_characters() {
    let mut server = Server::new();
    let pivot = server.mock("GET", Matcher::Any).expect(0).create();

    let out = redmine(&server)
        .args(["project", "view", "../../users/1.json?x=1#"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    pivot.assert();
}

#[test]
fn http_url_warns_in_auth_status() {
    let mut server = Server::new();
    server
        .mock("GET", "/users/current.json")
        .with_body(r#"{"user":{"id":1,"login":"tim"}}"#)
        .create();
    // mockito listens on 127.0.0.1: loopback http must not warn.
    let out = redmine(&server).args(["auth", "status"]).output().unwrap();
    assert!(out.status.success());
    assert!(stdout_json(&out).get("warning").is_none());
}

#[test]
fn json_escaped_key_is_scrubbed() {
    let mut server = Server::new();
    // The raw body never contains KEY; only the decoded JSON string does.
    // Built without a literal backslash so no tooling can pre-decode the escapes.
    let escaped: String = "123"
        .chars()
        .map(|c| format!("{}u{:04x}", char::from(92u8), c as u32))
        .collect();
    let body = format!(r#"{{"note":"test-key-{escaped}"}}"#);
    assert!(!body.contains(KEY), "{body}");
    server.mock("GET", "/echo.json").with_body(body).create();

    let out = redmine(&server)
        .args(["api", "/echo.json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(stdout_json(&out)["note"], "[REDACTED]");
}

#[test]
fn redirect_outside_base_path_is_not_followed() {
    let mut server = Server::new();
    let other_app = server.mock("GET", "/other-app/collect").expect(0).create();
    server
        .mock("GET", "/redmine/issues/1.json")
        .with_status(302)
        .with_header("location", "/other-app/collect")
        .create();

    let out = redmine(&server)
        .env("REDMINE_URL", format!("{}/redmine", server.url()))
        .args(["issue", "view", "1"])
        .output()
        .unwrap();
    assert_eq!(stderr_json(&out)["error"]["code"], "redirect");
    other_app.assert();
}

#[test]
fn all_stops_at_byte_cap() {
    let mut server = Server::new();
    let big = "x".repeat(1000);
    let items: Vec<Value> = (0..100).map(|i| json!({"id": i, "s": big})).collect();
    let pages = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .with_body(json!({"issues": items, "total_count": 1_000_000u64}).to_string())
        .expect(2)
        .create();

    let out = redmine(&server)
        .env("REDMINE_MAX_BYTES", "150000")
        .args(["issue", "list", "--all"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = stdout_json(&out);
    assert_eq!(v["truncated"], true);
    assert!(v["issues"].as_array().unwrap().len() < 200);
    pages.assert();
}

#[test]
fn api_path_cannot_escape_base_path() {
    let mut server = Server::new();
    let anything = server.mock("GET", Matcher::Any).expect(0).create();

    for path in [
        "../other-app/collect",
        "%2e%2e/other-app/collect",
        "issues/..%2f..%2fother-app/collect",
        "issues%5c..%5cother-app",
    ] {
        let out = redmine(&server)
            .env("REDMINE_URL", format!("{}/redmine", server.url()))
            .args(["api", path])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{path}");
        assert_eq!(stderr_json(&out)["error"]["code"], "usage", "{path}");
    }
    anything.assert();
}

#[test]
fn redirect_with_encoded_slash_is_not_followed() {
    let mut server = Server::new();
    let other_app = server
        .mock("GET", Matcher::Regex("other-app".into()))
        .expect(0)
        .create();
    server
        .mock("GET", "/redmine/issues/1.json")
        .with_status(302)
        .with_header("location", "/redmine/..%2f..%2fother-app/collect")
        .create();

    let out = redmine(&server)
        .env("REDMINE_URL", format!("{}/redmine", server.url()))
        .args(["issue", "view", "1"])
        .output()
        .unwrap();
    assert_eq!(stderr_json(&out)["error"]["code"], "redirect");
    other_app.assert();
}

#[test]
fn api_key_in_object_key_is_scrubbed() {
    let mut server = Server::new();
    server
        .mock("GET", "/x.json")
        .with_body(format!(r#"{{"note":{{"{KEY}":1}}}}"#))
        .create();

    let out = redmine(&server).args(["api", "/x.json"]).output().unwrap();
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains(KEY));
    assert_eq!(stdout_json(&out)["note"]["[REDACTED]"], 1);
}

#[test]
fn byte_cap_counts_memory_not_json_size() {
    // `[]` is 2 bytes of JSON but a full `Value` in memory.
    let mut server = Server::new();
    let items: Vec<Value> = (0..100)
        .map(|i| json!({"id": i, "a": vec![json!([]); 1000]}))
        .collect();
    server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .with_body(json!({"issues": items, "total_count": 1_000_000u64}).to_string())
        .create();

    let out = redmine(&server)
        .env("REDMINE_MAX_BYTES", "300000")
        .args(["issue", "list", "--all"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = stdout_json(&out);
    assert_eq!(v["truncated"], true);
    assert!(v["issues"].as_array().unwrap().len() < 50);
}

fn kv(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

fn long_filter(field: &str, op: &str, values: &[&str]) -> Vec<(String, String)> {
    let mut m = vec![kv("f[]", field), kv(&format!("op[{field}]"), op)];
    for v in values {
        m.push(kv(&format!("v[{field}][]"), v));
    }
    m
}

/// mockito's `UrlEncoded` keeps one value per key, but long-form filters repeat
/// keys (`f[]`, `v[field][]`), so compare the full list of decoded pairs.
fn query_contains(expected: Vec<(String, String)>) -> impl Fn(&mockito::Request) -> bool {
    move |req| {
        let pairs = query_pairs(req);
        expected.iter().all(|p| pairs.contains(p))
    }
}

fn query_pairs(req: &mockito::Request) -> Vec<(String, String)> {
    let query = req.path_and_query().split_once('?').map_or("", |(_, q)| q);
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

#[test]
fn issue_list_sends_long_form_filters() {
    // Some servers ignore `assigned_to_id=me`; every Redmine honours the long form.
    let mut server = Server::new();
    let mut m = vec![kv("set_filter", "1")];
    m.extend(long_filter("status_id", "c", &[]));
    m.extend(long_filter("assigned_to_id", "=", &["me"]));
    m.extend(long_filter("author_id", "=", &["7"]));
    m.extend(long_filter("updated_on", ">=", &["2026-01-01"]));
    m.extend(long_filter("cf_3", "=", &["a", "b c"]));
    let mock = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .match_request(query_contains(m))
        .with_body(r#"{"issues":[],"total_count":0,"offset":0,"limit":25}"#)
        .create();

    let out = redmine(&server)
        .args(["issue", "list", "-s", "closed", "-a", "me", "--author", "7"])
        .args(["-f", "updated_on >= 2026-01-01", "-f", "cf_3 = a|b c"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    mock.assert();
}

#[test]
fn issue_list_rejects_ambiguous_filters() {
    let server = Server::new();
    for args in [
        vec!["-f", "cf_3=foo"],
        vec!["-f", "cf_3 =foo"],
        vec!["-a", "someone"],
        vec!["--author", "someone"],
        vec!["--query", "4", "-a", "me"],
        vec!["--query", "4", "-s", "open"],
        vec!["--query", "4", "--author", "7"],
        vec!["--query", "4", "--tracker", "1"],
        vec!["--query", "4", "--priority", "1"],
        vec!["--query", "4", "-f", "cf_3 = 1"],
    ] {
        let out = redmine(&server)
            .args(["issue", "list"])
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn time_list_sends_long_form_filters() {
    let mut server = Server::new();
    let mut m = vec![kv("set_filter", "1"), kv("issue_id", "5")];
    m.extend(long_filter("user_id", "=", &["me"]));
    m.extend(long_filter("spent_on", "><", &["2026-10-01", "2026-10-31"]));
    let mock = server
        .mock("GET", "/time_entries.json")
        .match_query(Matcher::Any)
        .match_request(query_contains(m))
        .with_body(r#"{"time_entries":[],"total_count":0,"offset":0,"limit":25}"#)
        .create();

    let out = redmine(&server)
        .args(["time", "list", "-i", "5", "-u", "me"])
        .args(["--from", "2026-10-01", "--to", "2026-10-31"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    mock.assert();
}

#[test]
fn issue_search_uses_search_api() {
    let mut server = Server::new();
    let mock = server
        .mock("GET", "/projects/web/search.json")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("q".into(), "172.104.90.81".into()),
            Matcher::UrlEncoded("issues".into(), "1".into()),
            Matcher::UrlEncoded("open_issues".into(), "1".into()),
            Matcher::UrlEncoded("titles_only".into(), "1".into()),
        ]))
        .with_body(
            r#"{"results":[{"id":9,"title":"Bug #9 (New): x","type":"issue","url":"u","description":"d","datetime":"2026-01-01T00:00:00Z"}],"total_count":1,"offset":0,"limit":25}"#,
        )
        .create();

    let out = redmine(&server)
        .args([
            "issue",
            "search",
            "172.104.90.81",
            "-p",
            "web",
            "--open",
            "--titles-only",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = stdout_json(&out);
    assert_eq!(v["results"][0]["id"], 9);
    assert_eq!(v["search"], "fulltext");
    mock.assert();
}

#[test]
fn issue_search_falls_back_to_filters_without_search_api() {
    let mut server = Server::new();
    server
        .mock("GET", "/search.json")
        .match_query(Matcher::Any)
        .with_status(404)
        .create();
    let issue = |id: u64, subject: &str| {
        json!({"id": id, "subject": subject, "description": "has 10.0.0.1",
               "tracker": {"name": "Bug"}, "status": {"name": "New"},
               "created_on": "2026-01-01T00:00:00Z"})
    };
    let mut by = |field: &str, body: Value| {
        let mut m = vec![kv("set_filter", "1")];
        m.extend(long_filter("status_id", "*", &[]));
        m.extend(long_filter(field, "~", &["10.0.0.1"]));
        server
            .mock("GET", "/issues.json")
            .match_query(Matcher::Any)
            .match_request(query_contains(m))
            .with_body(body.to_string())
            .create()
    };
    let subject = by(
        "subject",
        json!({"issues": [issue(3, "10.0.0.1 down")], "total_count": 1}),
    );
    let description = by(
        "description",
        json!({"issues": [issue(5, "a"), issue(3, "10.0.0.1 down")], "total_count": 2}),
    );

    let out = redmine(&server)
        .args(["issue", "search", "10.0.0.1"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = stdout_json(&out);
    assert_eq!(v["search"], "subject_description");
    assert_eq!(v["total_count"], 2);
    let ids: Vec<u64> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [5, 3]);
    assert_eq!(v["results"][1]["title"], "Bug #3 (New): 10.0.0.1 down");
    subject.assert();
    description.assert();
}

#[test]
fn issue_search_rejects_path_characters_in_project() {
    let server = Server::new();
    let out = redmine(&server)
        .args(["issue", "search", "x", "-p", "../admin"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn query_matcher_rejects_wrong_requests() {
    // The long-form tests rely on query_contains; make sure it can say no.
    // The CLI sends set_filter=1, f[]=subject, op[subject]=~, v[subject][]=x, ...
    for expected in [
        vec![kv("op[subject]", "~"), kv("v[subject][]", "y")], // right key, wrong value
        vec![kv("op[status_id]", "~")],                        // right value, wrong key
        vec![kv("op[subject]", "=")],                          // wrong operator
    ] {
        let mut server = Server::new();
        let mock = server
            .mock("GET", "/issues.json")
            .match_query(Matcher::Any)
            .match_request(query_contains(expected.clone()))
            .with_body(r#"{"issues":[],"total_count":0}"#)
            .expect(0)
            .create();
        let out = redmine(&server)
            .args(["issue", "list", "-f", "subject ~ x"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{expected:?}");
        mock.assert();
    }
}

#[test]
fn issue_list_status_default_yields_to_query_and_f() {
    let mut server = Server::new();
    let saved = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .match_request(|req| {
            let pairs = query_pairs(req);
            pairs.contains(&kv("query_id", "4"))
                && pairs.contains(&kv("sort", "id"))
                && !pairs.iter().any(|(k, _)| k == "f[]" || k == "set_filter")
        })
        .with_body(r#"{"issues":[],"total_count":0}"#)
        .create();
    let out = redmine(&server)
        .args(["issue", "list", "--query", "4", "--sort", "id"])
        .output()
        .unwrap();
    assert!(out.status.success());
    saved.assert();

    let mut server = Server::new();
    let explicit = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .match_request(|req| {
            let pairs = query_pairs(req);
            pairs
                .iter()
                .filter(|p| **p == kv("f[]", "status_id"))
                .count()
                == 1
                && pairs.contains(&kv("op[status_id]", "="))
                && pairs.contains(&kv("v[status_id][]", "3"))
        })
        .with_body(r#"{"issues":[],"total_count":0}"#)
        .create();
    let out = redmine(&server)
        .args(["issue", "list", "-f", "status_id = 3"])
        .output()
        .unwrap();
    assert!(out.status.success());
    explicit.assert();
}

#[test]
fn issue_list_maps_each_flag_to_its_filter() {
    let mut server = Server::new();
    server
        .mock("GET", "/issue_statuses.json")
        .with_body(r#"{"issue_statuses":[{"id":3,"name":"Resolved"}]}"#)
        .create();
    let mut expect = |args: &[&str], mut expected: Vec<(String, String)>| {
        expected.push(kv("set_filter", "1"));
        let mock = server
            .mock("GET", "/issues.json")
            .match_query(Matcher::Any)
            .match_request(query_contains(expected))
            .with_body(r#"{"issues":[],"total_count":0}"#)
            .create();
        let out = redmine(&server)
            .args(["issue", "list"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        mock.assert();
        mock.remove();
    };
    expect(&["-s", "open"], long_filter("status_id", "o", &[]));
    expect(&["-s", "all"], long_filter("status_id", "*", &[]));
    let mut all = long_filter("status_id", "=", &["3"]);
    all.extend(long_filter("tracker_id", "=", &["4"]));
    all.extend(long_filter("priority_id", "=", &["5"]));
    all.extend(long_filter("author_id", "=", &["me"]));
    expect(
        &[
            "-s",
            "resolved",
            "--tracker",
            "4",
            "--priority",
            "5",
            "--author",
            "ME",
        ],
        all,
    );
}

#[test]
fn time_list_date_bounds_and_scope() {
    let mut server = Server::new();
    let mut expect = |args: &[&str], expected: Vec<(String, String)>| {
        let mock = server
            .mock("GET", "/time_entries.json")
            .match_query(Matcher::Any)
            .match_request(query_contains(expected))
            .with_body(r#"{"time_entries":[],"total_count":0}"#)
            .create();
        let out = redmine(&server)
            .args(["time", "list"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
        mock.assert();
        mock.remove();
    };
    expect(
        &["--from", "2026-10-01"],
        long_filter("spent_on", ">=", &["2026-10-01"]),
    );
    expect(
        &["--to", "2026-10-31"],
        long_filter("spent_on", "<=", &["2026-10-31"]),
    );
    let mut both = long_filter("spent_on", "><", &["2026-10-01", "2026-10-31"]);
    both.push(kv("project_id", "web"));
    expect(
        &["-p", "web", "--from", "2026-10-01", "--to", "2026-10-31"],
        both,
    );

    // "><" takes [from, to] in that order.
    let ordered = server
        .mock("GET", "/time_entries.json")
        .match_query(Matcher::Any)
        .match_request(|req| {
            let values: Vec<String> = query_pairs(req)
                .into_iter()
                .filter(|(k, _)| k == "v[spent_on][]")
                .map(|(_, v)| v)
                .collect();
            values == ["2026-10-01", "2026-10-31"]
        })
        .with_body(r#"{"time_entries":[],"total_count":0}"#)
        .create();
    let out = redmine(&server)
        .args(["time", "list", "--from", "2026-10-01", "--to", "2026-10-31"])
        .output()
        .unwrap();
    assert!(out.status.success());
    ordered.assert();

    let out = redmine(&server)
        .args(["time", "list", "-u", "someone"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// Fallback fixture: subject matches 5, 4, 3; description matches 6, 3, 2.
/// Issue 3 differs between the two so the test can tell which copy is kept.
fn fallback_server(subject_total: u64) -> ServerGuard {
    let mut server = Server::new();
    server
        .mock("GET", "/search.json")
        .match_query(Matcher::Any)
        .with_status(404)
        .create();
    let issue = |id: u64, subject: &str| {
        json!({"id": id, "subject": subject, "description": "d",
               "tracker": {"name": "Bug"}, "status": {"name": "New"},
               "created_on": format!("2026-01-0{id}T00:00:00Z"),
               "updated_on": "2026-12-31T00:00:00Z"})
    };
    for (field, ids, total) in [
        ("subject", [(5, "s5"), (4, "s4"), (3, "s3")], subject_total),
        ("description", [(6, "d6"), (3, "d3"), (2, "d2")], 3),
    ] {
        let issues: Vec<Value> = ids.iter().map(|(id, s)| issue(*id, s)).collect();
        server
            .mock("GET", "/issues.json")
            .match_query(Matcher::Any)
            .match_request(query_contains(long_filter(field, "~", &["q"])))
            .with_body(json!({"issues": issues, "total_count": total}).to_string())
            .create();
    }
    server
}

fn result_ids(v: &Value) -> Vec<u64> {
    v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_u64().unwrap())
        .collect()
}

#[test]
fn search_fallback_merges_and_pages_locally() {
    let server = fallback_server(3);
    let run = |args: &[&str]| {
        let out = redmine(&server)
            .args(["issue", "search", "q"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        stdout_json(&out)
    };

    let v = run(&["--limit", "2", "--offset", "1"]);
    assert_eq!(result_ids(&v), [5, 4]);
    assert_eq!(
        (v["total_count"].as_u64(), v["offset"].as_u64()),
        (Some(5), Some(1))
    );
    assert_eq!(v["limit"], 2);
    assert!(v.get("truncated").is_none());

    let v = run(&["--all", "--limit", "2"]);
    assert_eq!(result_ids(&v), [6, 5, 4, 3, 2]);
    assert_eq!(v["limit"], 5);
    // The subject match is queried first and kept.
    assert_eq!(v["results"][3]["title"], "Bug #3 (New): s3");
    assert_eq!(v["results"][3]["url"], format!("{}/issues/3", server.url()));
    assert_eq!(v["results"][3]["datetime"], "2026-01-03T00:00:00Z");

    assert_eq!(run(&["--limit", "150"])["limit"], 100);
}

#[test]
fn search_fallback_reports_truncation_from_either_query() {
    // Only the subject query has more pages than REDMINE_MAX_ITEMS allows.
    let server = fallback_server(99);
    let out = redmine(&server)
        .env("REDMINE_MAX_ITEMS", "3")
        .args(["issue", "search", "q"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(stdout_json(&out)["truncated"], true);
}

#[test]
fn search_fallback_fetches_every_page() {
    let mut server = Server::new();
    server
        .mock("GET", "/search.json")
        .match_query(Matcher::Any)
        .with_status(404)
        .create();
    let page = |ids: std::ops::Range<u64>| {
        let issues: Vec<Value> = ids.map(|id| json!({"id": id, "subject": "q"})).collect();
        json!({"issues": issues, "total_count": 101}).to_string()
    };
    for (offset, body) in [("0", page(1..101)), ("100", page(101..102))] {
        let mut m = long_filter("subject", "~", &["q"]);
        m.push(kv("offset", offset));
        server
            .mock("GET", "/issues.json")
            .match_query(Matcher::Any)
            .match_request(query_contains(m))
            .with_body(body)
            .create();
    }
    let out = redmine(&server)
        .args(["issue", "search", "q", "--titles-only", "--all"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = stdout_json(&out);
    assert_eq!(v["total_count"], 101);
    assert_eq!(result_ids(&v)[0], 101);
}

#[test]
fn search_fallback_respects_open_titles_only_and_project() {
    let mut server = Server::new();
    server
        .mock("GET", "/projects/web/search.json")
        .match_query(Matcher::Any)
        .with_status(404)
        .create();
    let description = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .match_request(query_contains(long_filter("description", "~", &["q"])))
        .expect(0)
        .create();
    let mut m = long_filter("subject", "~", &["q"]);
    m.extend(long_filter("status_id", "o", &[]));
    m.push(kv("project_id", "web"));
    let subject = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .match_request(query_contains(m))
        .with_body(r#"{"issues":[{"id":1,"subject":"q"}],"total_count":1}"#)
        .create();

    let out = redmine(&server)
        .args([
            "issue",
            "search",
            "q",
            "--open",
            "--titles-only",
            "-p",
            "web",
            "-o",
            "text",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("subject and description only"));
    subject.assert();
    description.assert();
}

#[test]
fn search_does_not_fall_back_on_other_errors() {
    let mut server = Server::new();
    server
        .mock("GET", "/search.json")
        .match_query(Matcher::Any)
        .with_status(500)
        .create();
    let issues = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .expect(0)
        .create();
    let out = redmine(&server)
        .args(["issue", "search", "q"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
    issues.assert();
}

#[test]
fn search_for_a_number_is_sent_as_a_phrase() {
    // Some Redmine releases answer /search.json?q=123 by jumping to issue 123;
    // a quoted phrase searches the same token on every version.
    let mut server = Server::new();
    let issues = server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .expect(0)
        .create();
    for text in ["123", "#123"] {
        let search = server
            .mock("GET", "/search.json")
            .match_query(Matcher::UrlEncoded("q".into(), format!("\"{text}\"")))
            .with_body(r#"{"results":[],"total_count":0,"offset":0,"limit":25}"#)
            .create();
        let out = redmine(&server)
            .args(["issue", "search", text])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{text}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(stdout_json(&out)["search"], "fulltext");
        search.assert();
    }
    issues.assert();
}

#[test]
fn search_in_unknown_project_reports_not_found_without_fallback_note() {
    let mut server = Server::new();
    server
        .mock("GET", "/projects/nope/search.json")
        .match_query(Matcher::Any)
        .with_status(404)
        .create();
    server
        .mock("GET", "/issues.json")
        .match_query(Matcher::Any)
        .with_status(404)
        .create();
    let out = redmine(&server)
        .args(["issue", "search", "q", "-p", "nope", "-o", "text"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("note:"));
}
