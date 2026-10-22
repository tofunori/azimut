use std::path::{Path, PathBuf};

/// Errors raised by azimut-io.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: not a {expected} file")]
    Format { path: PathBuf, expected: &'static str },
    #[error("{path}: {reason}")]
    Parse { path: PathBuf, reason: String },
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
