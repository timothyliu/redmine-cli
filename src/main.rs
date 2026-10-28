mod cli;
mod client;
mod commands;
mod context;
mod credentials;
mod error;
mod output;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    let out = output::Output::new(cli.global.format, cli.global.fields.clone());
    if let Err(err) = commands::run(cli, &out) {
        out.error(&err);
        std::process::exit(err.exit_code());
    }
}
