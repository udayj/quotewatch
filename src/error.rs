use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("application error: {0}")]
    App(#[from] crate::app::AppError),

    #[error("Gmail integration error: {0}")]
    Gmail(#[from] crate::integrations::gmail::GmailError),

    #[error("classifier integration error: {0}")]
    Classifier(#[from] crate::integrations::classifier::ClassifierError),

    #[error("prepared run error: {0}")]
    PreparedRun(#[from] crate::domain::prepared_run::PreparedRunError),

    #[error("database error: {0}")]
    Database(#[from] crate::storage::database::DatabaseError),

    #[error("failed to load .env: {0}")]
    Dotenv(#[from] dotenvy::Error),
}
