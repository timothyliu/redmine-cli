use super::lookup::Kind;
use crate::cli::MetaCommand;
use crate::context::Context;
use crate::error::Result;
use crate::output::Column;

pub fn run(ctx: &Context, cmd: MetaCommand) -> Result<()> {
    let (_, client) = ctx.client()?;
    let (kind, columns): (Kind, &[Column]) = match cmd {
        MetaCommand::Statuses => (
            Kind::Status,
            &[("ID", "id"), ("NAME", "name"), ("CLOSED", "is_closed")],
        ),
        MetaCommand::Trackers => (
            Kind::Tracker,
            &[
                ("ID", "id"),
                ("NAME", "name"),
                ("DEFAULT STATUS", "default_status.name"),
            ],
        ),
        MetaCommand::Priorities => (
            Kind::Priority,
            &[("ID", "id"), ("NAME", "name"), ("DEFAULT", "is_default")],
        ),
        MetaCommand::Activities => (
            Kind::Activity,
            &[("ID", "id"), ("NAME", "name"), ("DEFAULT", "is_default")],
        ),
    };
    let (path, key, _) = kind.endpoint();
    let value = client.get(path, &[])?;
    ctx.out.list(value, key, columns)
}
