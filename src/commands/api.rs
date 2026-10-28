use reqwest::Method;
use serde_json::{Value, json};

use super::{parse_kv, read_text, redact};
use crate::cli::ApiArgs;
use crate::context::Context;
use crate::error::{AppError, Result};
use crate::output::pretty;

pub fn run(ctx: &Context, args: ApiArgs) -> Result<()> {
    let method = Method::from_bytes(args.method.to_uppercase().as_bytes())
        .map_err(|_| AppError::Usage(format!("invalid HTTP method {:?}", args.method)))?;
    let body = read_text(args.data, args.data_file.as_deref())?
        .map(|raw| {
            serde_json::from_str::<Value>(&raw)
                .map_err(|e| AppError::Usage(format!("request body is not valid JSON: {e}")))
        })
        .transpose()?;
    let query = args
        .query
        .iter()
        .map(|kv| parse_kv(kv))
        .collect::<Result<Vec<_>>>()?;

    let (_, client) = ctx.client()?;
    let mut value = client
        .send(method, &args.path, &query, body.as_ref())?
        .unwrap_or_else(|| json!({ "ok": true }));
    redact(&mut value);
    ctx.out.object(value, pretty)
}
