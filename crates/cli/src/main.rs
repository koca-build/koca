#![allow(clippy::result_large_err)]

mod cli;
mod create;
mod error;
mod internal;
mod tui;

use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() {
    // Ctrl+C handler: restore terminal state before exit.
    // In raw mode SIGINT is suppressed, so we catch it via tokio.
    tokio::spawn(async {
        tokio::signal::ctrl_c().await.ok();
        crossterm::terminal::disable_raw_mode().ok();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
        std::process::exit(130);
    });

    let cli = Cli::parse();

    let output = match cli {
        Cli::Create(create_args) => create::run(create_args).await,
        Cli::Internal(args) => internal::run(args).await,
    };

    if let Err(errs) = output {
        for err in errs.0 {
            zolt::errln!("{:?}", anyhow::Error::from(err));
        }
    }
}
