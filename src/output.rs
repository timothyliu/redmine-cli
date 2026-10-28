use std::io::{self, IsTerminal, Write};

use clap::ValueEnum;
use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

use crate::error::{AppError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Text,
}

/// (header, dotted field path)
pub type Column = (&'static str, &'static str);

#[derive(Clone, Debug)]
pub struct Output {
    pub format: Format,
    fields: Option<Vec<String>>,
    pretty: bool,
}

impl Output {
    /// Defaults to text on a terminal and compact JSON otherwise (pipes, agents).
    pub fn new(format: Option<Format>, fields: Option<Vec<String>>) -> Self {
        let tty = io::stdout().is_terminal();
        let fields = fields
            .map(|f| {
                f.into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|f| !f.is_empty());
        Self {
            format: format.unwrap_or(if tty { Format::Text } else { Format::Json }),
            fields,
            pretty: tty,
        }
    }

    /// Prints a list response shaped like `{"<key>": [...], "total_count": ...}`.
    pub fn list(&self, mut value: Value, key: &str, columns: &[Column]) -> Result<()> {
        if let Some(fields) = &self.fields
            && let Some(items) = value.get_mut(key).and_then(Value::as_array_mut)
        {
            for item in items.iter_mut() {
                *item = project(item, fields);
            }
        }
        if self.format == Format::Json {
            return self.json(&value);
        }

        let items = value
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let cols: Vec<(String, String)> = match &self.fields {
            Some(f) => f.iter().map(|p| (p.to_uppercase(), p.clone())).collect(),
            None => columns
                .iter()
                .map(|(h, p)| (h.to_string(), p.to_string()))
                .collect(),
        };
        write_stdout(&table(items, &cols))?;
        if let Some(total) = value.get("total_count").and_then(Value::as_u64)
            && total > items.len() as u64
        {
            eprintln!(
                "showing {} of {total} (use --limit, --offset or --all for more)",
                items.len()
            );
        }
        Ok(())
    }

    /// Prints a single object; `render` produces the human-readable form.
    pub fn object(&self, value: Value, render: impl FnOnce(&Value) -> String) -> Result<()> {
        match (&self.fields, self.format) {
            (Some(f), Format::Json) => self.json(&project(&value, f)),
            (None, Format::Json) => self.json(&value),
            (Some(f), Format::Text) => {
                let lines: Vec<String> = f
                    .iter()
                    .map(|p| format!("{p}: {}", cell(get_path(&value, p))))
                    .collect();
                write_stdout(&lines.join("\n"))
            }
            (None, Format::Text) => write_stdout(render(&value).trim_end()),
        }
    }

    pub fn json(&self, value: &Value) -> Result<()> {
        let s = if self.pretty {
            serde_json::to_string_pretty(value)
        } else {
            serde_json::to_string(value)
        }
        .expect("serializing a Value cannot fail");
        write_stdout(&s)
    }

    pub fn error(&self, err: &AppError) {
        match self.format {
            Format::Json => eprintln!("{}", err.to_json()),
            Format::Text => eprintln!("error: {err}"),
        }
    }
}

pub fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("serializing a Value cannot fail")
}

fn write_stdout(s: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    match writeln!(out, "{s}").and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        // `redmine ... | head` closing the pipe is not an error.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => Err(e.into()),
    }
}

/// Looks up a dotted path such as `status.name` or `custom_fields.0.value`.
pub fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |cur, seg| match cur {
        Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => cur.get(seg),
    })
}

/// Keeps only `fields` (dotted paths), preserving nesting. Missing fields are omitted.
pub fn project(value: &Value, fields: &[String]) -> Value {
    let mut out = Value::Object(Map::new());
    for f in fields {
        if let Some(v) = get_path(value, f) {
            insert_path(&mut out, f, v.clone());
        }
    }
    out
}

fn insert_path(target: &mut Value, path: &str, leaf: Value) {
    let mut cur = target;
    let mut segs = path.split('.').peekable();
    while let Some(seg) = segs.next() {
        let Value::Object(map) = cur else { return };
        if segs.peek().is_none() {
            map.insert(seg.to_string(), leaf);
            return;
        }
        cur = map
            .entry(seg.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

/// One-line human rendering of a value; objects show their `name`.
pub fn cell(value: Option<&Value>) -> String {
    let s = match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| cell(Some(x)))
            .collect::<Vec<_>>()
            .join(", "),
        Some(v @ Value::Object(o)) => match o.get("name") {
            Some(n) => cell(Some(n)),
            None => v.to_string(),
        },
        Some(v) => v.to_string(),
    };
    s.replace(['\n', '\r', '\t'], " ")
}

fn table(items: &[Value], cols: &[(String, String)]) -> String {
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|it| cols.iter().map(|(_, p)| cell(get_path(it, p))).collect())
        .collect();
    let mut widths: Vec<usize> = cols.iter().map(|(h, _)| h.width()).collect();
    for row in &rows {
        for (w, c) in widths.iter_mut().zip(row) {
            *w = (*w).max(c.width());
        }
    }
    let fmt_row = |cells: &[&str]| -> String {
        let last = cells.len().saturating_sub(1);
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i == last {
                    c.to_string()
                } else {
                    format!("{c}{}", " ".repeat(widths[i] - c.width()))
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    let header: Vec<&str> = cols.iter().map(|(h, _)| h.as_str()).collect();
    let mut lines = vec![fmt_row(&header)];
    for row in &rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        lines.push(fmt_row(&cells));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(f: &[&str]) -> Vec<String> {
        f.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn get_path_walks_objects_and_arrays() {
        let v = json!({"status": {"name": "New"}, "cf": [{"value": "x"}]});
        assert_eq!(get_path(&v, "status.name"), Some(&json!("New")));
        assert_eq!(get_path(&v, "cf.0.value"), Some(&json!("x")));
        assert_eq!(get_path(&v, "status.missing"), None);
    }

    #[test]
    fn project_keeps_nesting_and_skips_missing() {
        let v = json!({"id": 1, "subject": "s", "status": {"id": 2, "name": "New"}});
        let p = project(&v, &fields(&["id", "status.name", "nope"]));
        assert_eq!(p, json!({"id": 1, "status": {"name": "New"}}));
    }

    #[test]
    fn project_merges_parent_and_child_paths() {
        let v = json!({"status": {"id": 2, "name": "New"}});
        let p = project(&v, &fields(&["status.name", "status.id"]));
        assert_eq!(p, json!({"status": {"name": "New", "id": 2}}));
    }

    #[test]
    fn cell_is_single_line_and_uses_names() {
        assert_eq!(cell(Some(&json!({"id": 3, "name": "Bug"}))), "Bug");
        assert_eq!(cell(Some(&json!("a\nb"))), "a b");
        assert_eq!(cell(None), "");
    }

    #[test]
    fn table_aligns_wide_characters() {
        let items = vec![
            json!({"id": 1, "subject": "中文"}),
            json!({"id": 22, "subject": "x"}),
        ];
        let cols = vec![
            ("SUBJECT".to_string(), "subject".to_string()),
            ("ID".to_string(), "id".to_string()),
        ];
        let t = table(&items, &cols);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "SUBJECT  ID");
        assert_eq!(lines[1], "中文     1");
        assert_eq!(lines[2], "x        22");
    }
}
