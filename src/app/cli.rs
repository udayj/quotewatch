use crate::reporting::report;
use chrono::NaiveDate;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Parser, Debug)]
#[command(about = "Gmail quotation and follow-up monitor")]
pub struct Cli {
    #[arg(long, default_value = "quotewatch.sqlite")]
    pub(crate) db: PathBuf,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    Run(RunArgs),
    Report(ReportArgs),
    ExportJson(ExportJsonArgs),
}

#[derive(Parser, Debug)]
pub(crate) struct RunArgs {
    #[arg(long)]
    pub(crate) date: Option<DateArg>,

    #[arg(long)]
    pub(crate) start: Option<DateArg>,

    #[arg(long)]
    pub(crate) end: Option<DateArg>,

    // Limits the max number of threads fetched from Gmail for a run.
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: usize,

    #[arg(long, default_value = "client_secret.json")]
    pub(crate) client_secret: PathBuf,

    #[arg(long, default_value = "token.json")]
    pub(crate) token: PathBuf,

    // Replays a previously exported Gmail snapshot instead of fetching live Gmail data.
    #[arg(long)]
    pub(crate) input_json: Option<PathBuf>,

    #[arg(long)]
    pub(crate) label: Option<String>,

    // Number of thread digests sent per classifier request; smaller batches reduce retry radius.
    #[arg(long, default_value_t = 10)]
    pub(crate) batch_size: usize,

    // Fetches and stores inputs without calling the classifier.
    #[arg(long)]
    pub(crate) skip_classify: bool,
}

#[derive(Parser, Debug)]
pub(crate) struct ReportArgs {
    #[arg(long, value_enum, default_value_t = ReportFormatArg::Concise)]
    pub(crate) format: ReportFormatArg,

    #[arg(long)]
    pub(crate) email: bool,

    #[arg(long, default_value = "client_secret.json")]
    pub(crate) client_secret: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ReportFormatArg {
    Concise,
    Detailed,
}

#[derive(Parser, Debug)]
pub(crate) struct ExportJsonArgs {
    #[arg(long)]
    pub(crate) date: DateArg,

    #[arg(long, default_value_t = 50)]
    pub(crate) limit: usize,

    #[arg(long, default_value = "client_secret.json")]
    pub(crate) client_secret: PathBuf,

    #[arg(long, default_value = "token.json")]
    pub(crate) token: PathBuf,

    #[arg(long)]
    pub(crate) out: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum DateArg {
    Date(NaiveDate),
    // Lets deployed wrappers request the current IST date without calculating it themselves.
    TodayIst,
}

impl FromStr for DateArg {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("today-ist") {
            return Ok(Self::TodayIst);
        }
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map(Self::Date)
            .map_err(|_| "expected YYYY-MM-DD or today-ist".to_string())
    }
}

impl From<ReportFormatArg> for report::ReportFormat {
    fn from(value: ReportFormatArg) -> Self {
        match value {
            ReportFormatArg::Concise => Self::Concise,
            ReportFormatArg::Detailed => Self::Detailed,
        }
    }
}
