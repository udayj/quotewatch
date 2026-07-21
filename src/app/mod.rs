pub mod cli;
pub mod error;

mod dates;
mod export;
mod preparation;
mod report_email;
mod runner;

pub use cli::Cli;
pub use error::AppError;

use crate::error::Result;

pub fn execute(cli: Cli) -> Result<()> {
    runner::execute(cli)
}
