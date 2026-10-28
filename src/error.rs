use serde_json::{Value, json};

pub type Result<T> = std::result::Result<T, AppError>;

/// Every failure maps to a stable machine-readable `code` and process exit code,
/// so callers (scripts, agents) can branch without parsing messages.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Config(String),
    #[error("credential store: {0}")]
    Credential(String),
    #[error("{message}")]
    Api {
        status: u16,
        message: String,
        details: Vec<String>,
    },
    #[error("{0}")]
    NotApplied(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Response(String),
    #[error("{0}")]
    Io(String),
}

/// (exit code, meaning) — also published by `redmine schema`.
pub const EXIT_CODES: &[(i32, &str)] = &[
    (0, "success"),
    (
        1,
        "unexpected error (I/O, unexpected HTTP status or response)",
    ),
    (2, "usage error (bad flags or arguments)"),
    (3, "missing configuration or credentials"),
    (
        4,
        "authentication rejected or permission denied (HTTP 401/403)",
    ),
    (5, "resource not found (HTTP 404)"),
    (
        6,
        "validation failed (HTTP 422) or a requested change was not applied",
    ),
    (7, "network failure or server error (HTTP 5xx)"),
];

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage",
            Self::Config(_) => "config",
            Self::Credential(_) => "credential",
            Self::Api { status, .. } => match status {
                401 => "unauthorized",
                403 => "forbidden",
                404 => "not_found",
                422 => "validation",
                500..=599 => "server",
                _ => "http",
            },
            Self::NotApplied(_) => "not_applied",
            Self::Network(_) => "network",
            Self::Response(_) => "unexpected_response",
            Self::Io(_) => "io",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Config(_) | Self::Credential(_) => 3,
            Self::Api { status, .. } => match status {
                401 | 403 => 4,
                404 => 5,
                422 => 6,
                500..=599 => 7,
                _ => 1,
            },
            Self::NotApplied(_) => 6,
            Self::Network(_) => 7,
            Self::Response(_) | Self::Io(_) => 1,
        }
    }

    pub fn to_json(&self) -> Value {
        let mut err = json!({ "code": self.code(), "message": self.to_string() });
        if let Self::Api {
            status, details, ..
        } = self
        {
            err["status"] = json!(status);
            err["details"] = json!(details);
        }
        json!({ "error": err })
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<keyring::Error> for AppError {
    fn from(e: keyring::Error) -> Self {
        Self::Credential(e.to_string())
    }
}
