//! JSONata standard library: 60+ built-in functions.
//!
//! Port of Go `functions/` package.

mod array;
mod boolean;
pub mod datetime;
mod eval_fn;
mod format_integer;
mod format_number;
mod hof;
mod numeric;
mod object;
mod parse_integer;
pub mod regex;
mod string_funcs;
mod types;

use std::rc::Rc;

use crate::evaluator::{BuiltinFn, Environment, FunctionValue};
use crate::value::Value;

/// Register all built-in functions into an environment.
pub fn register_all(env: &mut Environment) {
    // ── String ──────────────────────────────────────────────────────
    bind_signed_builtin(env, "string", string_funcs::fn_string, "x?b?");
    bind_builtin(env, "length", string_funcs::fn_length);
    bind_builtin(env, "substring", string_funcs::fn_substring);
    bind_builtin(env, "substringBefore", string_funcs::fn_substring_before);
    bind_builtin(env, "substringAfter", string_funcs::fn_substring_after);
    bind_signed_builtin(env, "uppercase", string_funcs::fn_uppercase, "s?");
    bind_signed_builtin(env, "lowercase", string_funcs::fn_lowercase, "s?");
    bind_builtin(env, "trim", string_funcs::fn_trim);
    bind_builtin(env, "pad", string_funcs::fn_pad);
    bind_builtin(env, "contains", string_funcs::fn_contains);
    bind_builtin(env, "split", string_funcs::fn_split);
    bind_builtin(env, "join", string_funcs::fn_join);
    bind_builtin(env, "base64encode", string_funcs::fn_base64_encode);
    bind_builtin(env, "base64decode", string_funcs::fn_base64_decode);
    bind_builtin(env, "encodeUrl", string_funcs::fn_encode_url);
    bind_builtin(
        env,
        "encodeUrlComponent",
        string_funcs::fn_encode_url_component,
    );
    bind_builtin(env, "decodeUrl", string_funcs::fn_decode_url);
    bind_builtin(
        env,
        "decodeUrlComponent",
        string_funcs::fn_decode_url_component,
    );

    // ── Numeric ─────────────────────────────────────────────────────
    bind_builtin(env, "number", numeric::fn_number);
    bind_builtin(env, "abs", numeric::fn_abs);
    bind_builtin(env, "floor", numeric::fn_floor);
    bind_builtin(env, "ceil", numeric::fn_ceil);
    bind_builtin(env, "round", numeric::fn_round);
    bind_builtin(env, "power", numeric::fn_power);
    bind_builtin(env, "sqrt", numeric::fn_sqrt);
    bind_builtin(env, "random", numeric::fn_random);
    bind_signed_builtin(env, "sum", numeric::fn_sum, "a<n>");
    bind_signed_builtin(env, "max", numeric::fn_max, "a<n>");
    bind_signed_builtin(env, "min", numeric::fn_min, "a<n>");
    bind_signed_builtin(env, "average", numeric::fn_average, "a<n>");
    bind_builtin(env, "formatBase", numeric::fn_format_base);
    bind_builtin(env, "formatNumber", format_number::fn_format_number);
    bind_builtin(env, "formatInteger", format_integer::fn_format_integer);
    bind_builtin(env, "parseInteger", parse_integer::fn_parse_integer);

    // ── Array ───────────────────────────────────────────────────────
    bind_builtin(env, "count", array::fn_count);
    bind_builtin(env, "append", array::fn_append);
    bind_builtin(env, "reverse", array::fn_reverse);
    bind_builtin(env, "shuffle", array::fn_shuffle);
    bind_builtin(env, "distinct", array::fn_distinct);
    bind_builtin(env, "flatten", array::fn_flatten);
    bind_builtin(env, "zip", array::fn_zip);

    // ── Object ──────────────────────────────────────────────────────
    bind_builtin(env, "keys", object::fn_keys);
    bind_builtin(env, "values", object::fn_values);
    bind_builtin(env, "spread", object::fn_spread);
    bind_builtin(env, "merge", object::fn_merge);
    bind_builtin(env, "lookup", object::fn_lookup);
    bind_builtin(env, "error", object::fn_error);

    // ── Boolean ─────────────────────────────────────────────────────
    bind_signed_builtin(env, "boolean", boolean::fn_boolean, "x?");
    bind_builtin(env, "not", boolean::fn_not);
    bind_builtin(env, "exists", boolean::fn_exists);

    // ── Type / Misc ─────────────────────────────────────────────────
    bind_builtin(env, "type", types::fn_type_of);
    bind_builtin(env, "assert", types::fn_assert);

    // ── HOF (env-aware) ─────────────────────────────────────────────
    bind_env_builtin(env, "map", hof::fn_map);
    bind_env_builtin(env, "filter", hof::fn_filter);
    bind_env_builtin(env, "reduce", hof::fn_reduce);
    bind_env_builtin(env, "each", hof::fn_each);
    bind_env_builtin(env, "sift", hof::fn_sift);
    bind_env_builtin(env, "sort", hof::fn_sort);
    bind_env_builtin(env, "single", hof::fn_single);

    // ── Regex / Pattern ──────────────────────────────────────────────
    bind_env_builtin(env, "match", regex::fn_match);
    bind_env_builtin(env, "replace", regex::fn_replace);
    bind_env_builtin(env, "eval", eval_fn::fn_eval);

    // ── DateTime ────────────────────────────────────────────────────
    bind_builtin(env, "now", datetime::fn_now);
    bind_builtin(env, "millis", datetime::fn_millis);
    bind_builtin(env, "fromMillis", datetime::fn_from_millis);
    bind_builtin(env, "toMillis", datetime::fn_to_millis);
}

/// Register stdlib on an Rc<Environment> (for $eval child envs).
pub fn register_all_on_rc(env: &Rc<Environment>) {
    // String
    env.bind("string", _mk_sb(string_funcs::fn_string, "x-b?"));
    env.bind("length", _mk_b(string_funcs::fn_length));
    env.bind("uppercase", _mk_sb(string_funcs::fn_uppercase, "s-"));
    env.bind("lowercase", _mk_sb(string_funcs::fn_lowercase, "s-"));
    env.bind("trim", _mk_b(string_funcs::fn_trim));
    env.bind("contains", _mk_b(string_funcs::fn_contains));
    env.bind("split", _mk_b(string_funcs::fn_split));
    env.bind("join", _mk_b(string_funcs::fn_join));
    // Numeric
    env.bind("number", _mk_b(numeric::fn_number));
    env.bind("abs", _mk_b(numeric::fn_abs));
    env.bind("floor", _mk_b(numeric::fn_floor));
    env.bind("ceil", _mk_b(numeric::fn_ceil));
    env.bind("round", _mk_b(numeric::fn_round));
    env.bind("sum", _mk_sb(numeric::fn_sum, "a<n>"));
    env.bind(
        "formatInteger",
        _mk_b(format_integer::fn_format_integer),
    );
    env.bind(
        "parseInteger",
        _mk_b(parse_integer::fn_parse_integer),
    );
    env.bind("count", _mk_b(array::fn_count));
    env.bind("append", _mk_b(array::fn_append));
    env.bind("keys", _mk_b(object::fn_keys));
    env.bind("values", _mk_b(object::fn_values));
    env.bind("boolean", _mk_sb(boolean::fn_boolean, "x-"));
    env.bind("not", _mk_b(boolean::fn_not));
    env.bind("exists", _mk_b(boolean::fn_exists));
    env.bind("type", _mk_b(types::fn_type_of));
    // HOF
    env.bind("map", _mk_e(hof::fn_map));
    env.bind("filter", _mk_e(hof::fn_filter));
    env.bind("reduce", _mk_e(hof::fn_reduce));
    env.bind("sort", _mk_e(hof::fn_sort));
}

fn _mk_b(f: fn(&[Value], &Value) -> crate::error::JsonataResult) -> Value {
    Value::Function(Box::new(FunctionValue::Builtin(Rc::new(f))))
}

fn _mk_sb(f: fn(&[Value], &Value) -> crate::error::JsonataResult, sig: &str) -> Value {
    Value::Function(Box::new(FunctionValue::SignedBuiltin {
        func: Rc::new(f),
        signature: sig.into(),
    }))
}

fn _mk_e(
    f: fn(
        &[Value],
        &Value,
        &Rc<Environment>,
        &crate::parser::AstArena,
    ) -> crate::error::JsonataResult,
) -> Value {
    Value::Function(Box::new(FunctionValue::EnvAwareBuiltin(Rc::new(f))))
}

fn bind_builtin(
    env: &mut Environment,
    name: &str,
    f: fn(&[Value], &Value) -> crate::error::JsonataResult,
) {
    let func: Rc<BuiltinFn> = Rc::new(f);
    env.bind(name, Value::Function(Box::new(FunctionValue::Builtin(func))));
}

fn bind_signed_builtin(
    env: &mut Environment,
    name: &str,
    f: fn(&[Value], &Value) -> crate::error::JsonataResult,
    signature: &str,
) {
    let func: Rc<BuiltinFn> = Rc::new(f);
    env.bind(
        name,
        Value::Function(Box::new(FunctionValue::SignedBuiltin {
            func,
            signature: signature.into(),
        })),
    );
}

fn bind_env_builtin(
    env: &mut Environment,
    name: &str,
    f: fn(
        &[Value],
        &Value,
        &Rc<Environment>,
        &crate::parser::AstArena,
    ) -> crate::error::JsonataResult,
) {
    let func: Rc<crate::evaluator::EnvAwareBuiltinFn> = Rc::new(f);
    env.bind(
        name,
        Value::Function(Box::new(FunctionValue::EnvAwareBuiltin(func))),
    );
}
