use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    Parse(String),
    Corrupt(String),
    Unsupported(String),
    InvalidPageIndex(usize),
    MissingObject(String),
    UnsupportedOption(String),
}

pub type PdfResult<T> = Result<T, PdfError>;

impl Display for PdfError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::Parse(message) => write!(f, "parse error: {message}"),
            PdfError::Corrupt(message) => write!(f, "corrupt pdf: {message}"),
            PdfError::Unsupported(message) => write!(f, "unsupported feature: {message}"),
            PdfError::InvalidPageIndex(index) => write!(f, "invalid page index: {index}"),
            PdfError::MissingObject(message) => write!(f, "missing object: {message}"),
            PdfError::UnsupportedOption(message) => write!(f, "unsupported option: {message}"),
        }
    }
}

impl Error for PdfError {}
