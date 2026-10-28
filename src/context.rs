use crate::cli::GlobalArgs;
use crate::client::Client;
use crate::credentials::{self, Credentials};
use crate::error::{AppError, Result};
use crate::output::Output;

pub struct Context {
    pub out: Output,
    pub profile: String,
    pub dry_run: bool,
}

impl Context {
    pub fn new(global: &GlobalArgs, out: Output) -> Self {
        Self {
            out,
            profile: global.profile.clone(),
            dry_run: global.dry_run,
        }
    }

    pub fn credentials(&self) -> Result<Credentials> {
        credentials::resolve(&self.profile)
    }

    /// Returns the base URL alongside an authenticated client.
    pub fn client(&self) -> Result<(String, Client)> {
        let creds = self.credentials()?;
        let (key, _) = creds.api_key.ok_or_else(|| {
            AppError::Config(format!(
                "no API key for {}: set {} or run `redmine auth login`",
                creds.url,
                credentials::ENV_API_KEY
            ))
        })?;
        let client = Client::new(&creds.url, key, self.dry_run)?;
        Ok((creds.url, client))
    }
}
