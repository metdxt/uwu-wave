//! Error type.
use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Wav(hound::Error),
    /// Routing problem (cycles, unknown sidechain source...).
    Graph(String),
    /// Song/pattern/sample definition problem.
    Invalid(String),
    /// The song's instruments were already consumed by a previous render.
    AlreadyRendered,
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::Wav(e) => write!(f, "wav error: {e}"),
            Error::Graph(s) => write!(f, "routing error: {s}"),
            Error::Invalid(s) => write!(f, "invalid: {s}"),
            Error::AlreadyRendered => write!(f, "this song was already rendered (instruments are consumed by a render)"),
            Error::Other(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}
impl From<hound::Error> for Error {
    fn from(e: hound::Error) -> Error {
        Error::Wav(e)
    }
}
impl From<String> for Error {
    fn from(s: String) -> Error {
        Error::Other(s)
    }
}
impl From<&str> for Error {
    fn from(s: &str) -> Error {
        Error::Other(s.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
