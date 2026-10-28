use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};

use crate::output::Format;

const LONG_ABOUT: &str = "Agent-friendly command-line client for Redmine.

Output is compact JSON when stdout is not a terminal (or with --format json).
Errors are JSON on stderr with stable exit codes. Run `redmine schema` for a
machine-readable description of every command, flag and exit code.

Credentials (URL and API key), in lookup order:
  1. REDMINE_URL / REDMINE_API_KEY environment variables
  2. the system keychain (macOS Keychain), saved with `redmine auth login`";

#[derive(Parser, Debug)]
#[command(name = "redmine", version, about = "Agent-friendly command-line client for Redmine", long_about = LONG_ABOUT)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Debug)]
pub struct GlobalArgs {
    /// Keychain profile holding the URL and API key
    #[arg(long, global = true, env = "REDMINE_PROFILE", value_name = "NAME", default_value = crate::credentials::DEFAULT_PROFILE)]
    pub profile: String,
    /// Output format [default: text on a terminal, json otherwise]
    #[arg(long, short = 'o', global = true, env = "REDMINE_FORMAT", value_enum)]
    pub format: Option<Format>,
    /// Only output these fields (comma-separated dotted paths, e.g. id,subject,status.name)
    #[arg(long, global = true, value_delimiter = ',', value_name = "FIELDS")]
    pub fields: Option<Vec<String>>,
    /// Print write requests (POST/PUT/DELETE) instead of sending them
    #[arg(long, global = true)]
    pub dry_run: bool,
}

// Parsed once per process; boxing variants would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Save, inspect or remove the Redmine URL and API key
    #[command(subcommand)]
    Auth(AuthCommand),
    /// List, view, create, update and comment on issues
    #[command(subcommand)]
    Issue(IssueCommand),
    /// List and view projects
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Show users
    #[command(subcommand)]
    User(UserCommand),
    /// List and log time entries
    #[command(subcommand)]
    Time(TimeCommand),
    /// List statuses, trackers, priorities and activities (id <-> name lookups)
    #[command(subcommand)]
    Meta(MetaCommand),
    /// Make an authenticated request to any Redmine REST endpoint
    Api(ApiArgs),
    /// Print a machine-readable description of commands, flags and exit codes
    Schema(SchemaArgs),
}

#[derive(Subcommand, Debug)]
pub enum AuthCommand {
    /// Verify and save the URL and API key to the keychain
    Login(LoginArgs),
    /// Remove the profile from the keychain
    Logout,
    /// Show where the URL and API key come from, and verify them
    Status,
}

#[derive(Args, Debug)]
pub struct LoginArgs {
    /// Redmine base URL [default: REDMINE_URL, else prompt]
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,
    /// Read the API key from stdin instead of prompting
    #[arg(long)]
    pub with_token: bool,
    /// Save without verifying the key against the server
    #[arg(long)]
    pub no_verify: bool,
}

#[derive(Args, Debug, Clone)]
pub struct PageArgs {
    /// Maximum number of records (Redmine caps this at 100)
    #[arg(long, default_value_t = 25)]
    pub limit: u32,
    /// Number of records to skip
    #[arg(long, default_value_t = 0)]
    pub offset: u32,
    /// Fetch every page (ignores --limit)
    #[arg(long)]
    pub all: bool,
}

#[derive(Subcommand, Debug)]
pub enum IssueCommand {
    /// List issues
    List(IssueListArgs),
    /// Show one issue
    View(IssueViewArgs),
    /// Create an issue
    Create(IssueCreateArgs),
    /// Update an issue (fields and/or a note)
    Update(IssueUpdateArgs),
    /// Add a comment (journal note) to an issue
    Comment(IssueCommentArgs),
}

#[derive(Args, Debug)]
pub struct IssueListArgs {
    /// Project id or identifier
    #[arg(long, short)]
    pub project: Option<String>,
    /// open, closed, all, or a status name/id [default: open, unless --query is given]
    #[arg(long, short)]
    pub status: Option<String>,
    /// Assignee user id, or "me"
    #[arg(long, short)]
    pub assignee: Option<String>,
    /// Author user id, or "me"
    #[arg(long)]
    pub author: Option<String>,
    /// Tracker name or id
    #[arg(long, short)]
    pub tracker: Option<String>,
    /// Priority name or id
    #[arg(long)]
    pub priority: Option<String>,
    /// Saved query id
    #[arg(long)]
    pub query: Option<u64>,
    /// Sort order, e.g. "updated_on:desc" or "priority:desc,id"
    #[arg(long)]
    pub sort: Option<String>,
    /// Raw filter passed to /issues.json (repeatable), e.g. cf_3=foo or "updated_on=>=2024-01-01"
    #[arg(long = "filter", short = 'f', value_name = "KEY=VALUE")]
    pub filters: Vec<String>,
    #[command(flatten)]
    pub page: PageArgs,
}

#[derive(Args, Debug)]
pub struct IssueViewArgs {
    pub id: u64,
    /// Extra associations to include (comma-separated)
    #[arg(long, value_delimiter = ',', value_parser = ["children", "attachments", "relations", "changesets", "journals", "watchers", "allowed_statuses"])]
    pub include: Vec<String>,
}

/// Fields shared by `issue create` and `issue update`.
#[derive(Args, Debug)]
pub struct IssueFields {
    /// Description text
    #[arg(long, conflicts_with = "description_file")]
    pub description: Option<String>,
    /// Read the description from a file ("-" for stdin)
    #[arg(long, value_name = "PATH")]
    pub description_file: Option<PathBuf>,
    /// Tracker name or id
    #[arg(long, short)]
    pub tracker: Option<String>,
    /// Status name or id
    #[arg(long, short)]
    pub status: Option<String>,
    /// Priority name or id
    #[arg(long)]
    pub priority: Option<String>,
    /// Assignee user id, "me", or "none" to unassign
    #[arg(long, short)]
    pub assignee: Option<String>,
    /// Parent issue id, or "none" to clear
    #[arg(long)]
    pub parent: Option<String>,
    /// Start date (YYYY-MM-DD)
    #[arg(long)]
    pub start_date: Option<String>,
    /// Due date (YYYY-MM-DD)
    #[arg(long)]
    pub due_date: Option<String>,
    /// Estimated hours
    #[arg(long)]
    pub estimated_hours: Option<f64>,
    /// Percent done (0-100)
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
    pub done_ratio: Option<u8>,
    /// Custom field as ID=VALUE (repeatable)
    #[arg(long = "custom-field", value_name = "ID=VALUE")]
    pub custom_fields: Vec<String>,
}

#[derive(Args, Debug)]
pub struct IssueCreateArgs {
    /// Project id or identifier
    #[arg(long, short)]
    pub project: String,
    /// Subject (title)
    #[arg(long)]
    pub subject: String,
    #[command(flatten)]
    pub fields: IssueFields,
    /// Watcher user id or "me" (repeatable)
    #[arg(long = "watcher", value_name = "USER")]
    pub watchers: Vec<String>,
}

#[derive(Args, Debug)]
pub struct IssueUpdateArgs {
    pub id: u64,
    /// Subject (title)
    #[arg(long)]
    pub subject: Option<String>,
    #[command(flatten)]
    pub fields: IssueFields,
    /// Add a note along with the change
    #[arg(long, conflicts_with = "notes_file")]
    pub notes: Option<String>,
    /// Read the note from a file ("-" for stdin)
    #[arg(long, value_name = "PATH")]
    pub notes_file: Option<PathBuf>,
    /// Make the note private
    #[arg(long)]
    pub private_notes: bool,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("source").required(true).args(["body", "body_file"])))]
pub struct IssueCommentArgs {
    pub id: u64,
    /// Comment text
    #[arg(long, short)]
    pub body: Option<String>,
    /// Read the comment from a file ("-" for stdin)
    #[arg(long, value_name = "PATH")]
    pub body_file: Option<PathBuf>,
    /// Make the comment private
    #[arg(long)]
    pub private: bool,
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    /// List projects
    List(PageArgs),
    /// Show one project
    View {
        /// Project id or identifier
        id: String,
        /// Extra associations to include (comma-separated)
        #[arg(long, value_delimiter = ',', value_parser = ["trackers", "issue_categories", "enabled_modules", "time_entry_activities", "issue_custom_fields"])]
        include: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum UserCommand {
    /// Show the authenticated user
    Me,
    /// Show a user by id
    View { id: u64 },
}

#[derive(Subcommand, Debug)]
pub enum TimeCommand {
    /// List time entries
    List(TimeListArgs),
    /// Log time on an issue or project
    Log(TimeLogArgs),
}

#[derive(Args, Debug)]
pub struct TimeListArgs {
    /// Project id or identifier
    #[arg(long, short)]
    pub project: Option<String>,
    /// Issue id
    #[arg(long, short)]
    pub issue: Option<u64>,
    /// User id, or "me"
    #[arg(long, short)]
    pub user: Option<String>,
    /// From date (YYYY-MM-DD, inclusive)
    #[arg(long)]
    pub from: Option<String>,
    /// To date (YYYY-MM-DD, inclusive)
    #[arg(long)]
    pub to: Option<String>,
    #[command(flatten)]
    pub page: PageArgs,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("target").required(true).args(["issue", "project"])))]
pub struct TimeLogArgs {
    /// Issue id
    #[arg(long, short)]
    pub issue: Option<u64>,
    /// Project id or identifier
    #[arg(long, short)]
    pub project: Option<String>,
    /// Hours spent
    #[arg(long = "hours", short = 'H')]
    pub hours: f64,
    /// Activity name or id [default: the server's default activity]
    #[arg(long, short)]
    pub activity: Option<String>,
    /// Comment
    #[arg(long, short)]
    pub comment: Option<String>,
    /// Date spent (YYYY-MM-DD) [default: today, server time]
    #[arg(long)]
    pub date: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum MetaCommand {
    /// Issue statuses
    Statuses,
    /// Trackers
    Trackers,
    /// Issue priorities
    Priorities,
    /// Time entry activities
    Activities,
}

#[derive(Args, Debug)]
pub struct ApiArgs {
    /// Path relative to the Redmine URL, e.g. /issues/1.json (must stay under it)
    pub path: String,
    /// HTTP method
    #[arg(long, short = 'X', default_value = "GET")]
    pub method: String,
    /// JSON request body
    #[arg(long, short, conflicts_with = "data_file")]
    pub data: Option<String>,
    /// Read the JSON request body from a file ("-" for stdin)
    #[arg(long, value_name = "PATH")]
    pub data_file: Option<PathBuf>,
    /// Query parameter as KEY=VALUE (repeatable)
    #[arg(long = "query", short, value_name = "KEY=VALUE")]
    pub query: Vec<String>,
}

#[derive(Args, Debug)]
pub struct SchemaArgs {
    /// Only describe this subcommand, e.g. `issue create`
    pub command: Vec<String>,
}
