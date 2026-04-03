// Pedantic by default, with targeted allows
#![warn(clippy::pedantic)]
// Cast family -- too noisy for f64-based numeric engine
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
// Not useful during active development
#![allow(clippy::doc_markdown)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::used_underscore_items)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::float_cmp)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::many_single_char_names)]
// Restriction lints -- opt in
#![warn(clippy::unwrap_used)]
#![warn(clippy::panic)]

pub mod error;
pub mod evaluator;
pub mod lexer;
pub mod parser;
pub mod stdlib;
pub mod value;

pub use error::JsonataError;
pub use evaluator::{Environment, FunctionValue, eval};
pub use lexer::{Lexer, Token, TokenType};
pub use parser::{AstArena, Expr, NodeId, Parser, process_ast};
pub use value::Value;
