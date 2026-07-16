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
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]

// Modules are crate-private; the supported public API is the curated set of
// re-exports below. `wasm` stays public — it is the wasm-bindgen surface.
pub(crate) mod error;
pub(crate) mod evaluator;
pub(crate) mod expression;
pub(crate) mod fast_path;
pub(crate) mod formatter;
pub(crate) mod highlight;
pub(crate) mod lexer;
pub(crate) mod parser;
pub(crate) mod stdlib;
pub(crate) mod stream;
pub(crate) mod value;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub mod wasm;

pub use error::JsonataError;
pub use evaluator::Environment;
pub use expression::{CustomFunc, Expression, new_custom_env};
pub use fast_path::FastPath;
pub use formatter::format;
pub use highlight::highlight;
pub use stream::{MetricsHook, StreamEvaluator, StreamStats};
pub use value::{CompareOp, ObjectMap, Value};

// Internal items re-exported for the in-repo bins, benches, integration
// tests, and fuzz targets (all separate crates), and to keep types named in
// hidden `Value` variants publicly reachable. Not part of the supported API —
// no stability guarantees.
#[doc(hidden)]
pub use evaluator::{BuiltinFn, EnvAwareBuiltinFn, FunctionValue, Lambda, TailCall};
#[doc(hidden)]
pub use evaluator::{ParamSpec, eval, parse_signature};
#[doc(hidden)]
pub use fast_path::testing as fast_path_testing;
#[doc(hidden)]
pub use lexer::{Lexer, Token, TokenType};
#[doc(hidden)]
pub use parser::{AstArena, Expr, NodeId, Parser, process_ast};
#[doc(hidden)]
pub use stdlib::register_all;
#[doc(hidden)]
pub use value::{Sequence, format_float};

/// Remaining-stack threshold (bytes) below which `stacker::maybe_grow`
/// allocates a new segment before recursing.
///
/// 128 KiB comfortably covers the deepest run of parser/evaluator frames
/// between two stack checks, so growth triggers before genuine overflow.
/// Absent on wasm32, where the stack cannot be grown.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const STACK_RED_ZONE: usize = 128 * 1024;

/// Size (bytes) of each stack segment `stacker::maybe_grow` allocates.
///
/// 1 MiB holds thousands of recursion frames per segment, amortizing
/// allocation cost across deeply nested expressions.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const STACK_GROW_SIZE: usize = 1024 * 1024;

/// Compare two values with JSONata equality semantics.
///
/// Equivalent to Go's `gnata.DeepEqual(a, b)`. Follows JSONata rules:
/// `undefined = undefined` is `false`, `null = null` is `true`,
/// numbers compare by value, arrays/objects compare recursively.
pub fn deep_equal(a: &Value, b: &Value) -> bool {
    a.deep_equal(b)
}
