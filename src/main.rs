mod app;
mod domain;
mod error;
mod integrations;
mod reporting;
mod storage;

use app::Cli;
use clap::Parser;
use error::Result;
use std::io::ErrorKind;

fn main() -> Result<()> {
    match dotenvy::dotenv() {
        Ok(_) => {}
        // .env is optional; hosted runs provide configuration through the process environment.
        Err(dotenvy::Error::Io(err)) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let cli = Cli::parse();
    app::execute(cli)
}
