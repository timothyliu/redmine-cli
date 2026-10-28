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
