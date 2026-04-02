use std::fmt;

/// Structured error type matching JSONata spec error codes.
///
/// Code prefixes:
/// - S0xxx: Syntax errors (lexer/parser)
/// - T0xxx: Type errors (function arguments)
/// - T1xxx: Type errors (function-specific)
/// - T2xxx: Type errors (operators)
/// - D1xxx: Domain errors (numeric)
/// - D2xxx: Domain errors (general)
/// - D3xxx: Domain errors (function-specific)
/// - U1001: Stack overflow
#[derive(Debug, Clone)]
pub struct JsonataError {
    pub code: String,
    pub token: String,
    pub value: Option<String>,
    pub message: String,
}

impl JsonataError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            token: String::new(),
            value: None,
            message: message.into(),
        }
    }

    pub fn with_code(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            token: String::new(),
            value: None,
            message: String::new(),
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = token.into();
        self
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }
}

impl fmt::Display for JsonataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (!self.message.is_empty(), !self.code.is_empty()) {
            (true, true) => write!(f, "{}: {}", self.code, self.message),
            (true, false) => write!(f, "{}", self.message),
            (false, true) => write!(f, "{}", self.code),
            (false, false) => write!(f, "unknown error"),
        }
    }
}

impl std::error::Error for JsonataError {}

/// Result type alias used throughout evaluation.
pub type JsonataResult<T = super::Value> = Result<T, JsonataError>;
