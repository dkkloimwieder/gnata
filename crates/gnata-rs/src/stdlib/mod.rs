//! JSONata standard library: 60+ built-in functions.
//!
//! Port of Go `functions/` package.

mod array;
mod boolean;
mod hof;
mod numeric;
mod object;
mod string_funcs;
mod types;

use std::rc::Rc;

use crate::evaluator::{BuiltinFn, Environment, FunctionValue};
use crate::value::Value;

/// Register all built-in functions into an environment.
pub fn register_all(env: &mut Environment) {
    // ── String ──────────────────────────────────────────────────────
    bind_builtin(env, "string", string_funcs::fn_string);
    bind_builtin(env, "length", string_funcs::fn_length);
    bind_builtin(env, "substring", string_funcs::fn_substring);
    bind_builtin(env, "substringBefore", string_funcs::fn_substring_before);
    bind_builtin(env, "substringAfter", string_funcs::fn_substring_after);
    bind_builtin(env, "uppercase", string_funcs::fn_uppercase);
    bind_builtin(env, "lowercase", string_funcs::fn_lowercase);
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
    bind_builtin(env, "sum", numeric::fn_sum);
    bind_builtin(env, "max", numeric::fn_max);
    bind_builtin(env, "min", numeric::fn_min);
    bind_builtin(env, "average", numeric::fn_average);
    bind_builtin(env, "formatBase", numeric::fn_format_base);

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
    bind_builtin(env, "boolean", boolean::fn_boolean);
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

    // ── DateTime ────────────────────────────────────────────────────
    bind_builtin(env, "now", types::fn_now);
    bind_builtin(env, "millis", types::fn_millis);
}

fn bind_builtin(
    env: &mut Environment,
    name: &str,
    f: fn(&[Value], &Value) -> crate::error::JsonataResult,
) {
    let func: Rc<BuiltinFn> = Rc::new(f);
    env.bind(name.into(), Value::Function(FunctionValue::Builtin(func)));
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
        name.into(),
        Value::Function(FunctionValue::EnvAwareBuiltin(func)),
    );
}
