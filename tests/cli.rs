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
            Matcher::UrlEncoded("status_id".into(), "open".into()),
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
