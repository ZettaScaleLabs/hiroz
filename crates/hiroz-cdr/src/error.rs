//! Error types for CDR serialization/deserialization

use std::fmt::Display;

use serde::{de, ser};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Custom(String),

    #[error("cannot deserialize unknown type in CDR")]
    UnsupportedAny,

    #[error("sequence length required")]
    UnknownLength,

    #[error("unexpected end of input")]
    UnexpectedEof,

    #[error("invalid boolean value: {0}")]
    InvalidBool(u8),

    #[error("invalid char codepoint: {0:#x}")]
    InvalidChar(u32),

    #[error("invalid option discriminant: {0}")]
    InvalidOptionTag(u32),

    #[error("{0}")]
    Utf8(#[from] std::str::Utf8Error),
}

impl ser::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Self::Custom(msg.to_string())
    }
}

impl de::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        Self::Custom(msg.to_string())
    }
}
