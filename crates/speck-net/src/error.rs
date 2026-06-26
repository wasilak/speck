use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Netstack error: {0}")]
    Netstack(String),

    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
