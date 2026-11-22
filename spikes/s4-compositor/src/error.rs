//! Crate-wide error type.

/// Everything that can go wrong in the spike, as one flat enum.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no GPU adapter matched `{0}`")]
    NoAdapter(String),
    #[error("GPU device request failed: {0}")]
    RequestDevice(String),
    #[error("GPU error: {0}")]
    Gpu(String),
    #[error("GPU device lost: {0}")]
    DeviceLost(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("unknown source `{0}`")]
    UnknownSource(String),
    #[error("unknown LUT `{0}`")]
    UnknownLut(String),
    #[error("invalid scene: {0}")]
    InvalidScene(String),
    #[error("invalid render graph: {0}")]
    InvalidGraph(String),
    #[error(".cube parse error at line {line}: {msg}")]
    Cube { line: usize, msg: String },
    #[error("image: {0}")]
    Image(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;
