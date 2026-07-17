# gnata

A [JSONata](https://jsonata.org) 2.x query and transformation engine for
Rust, ported from the Go implementation of the same name. It passes all
1,733 cases of the ported JSONata conformance suite, preserves the
reference implementation's semantics (`undefined` vs `null`, sequence
collapsing, insertion-ordered objects, ECMAScript number formatting), and
compiles to a compact WebAssembly module.

## Quick start

```rust
use gnata::Expression;

fn main() -> Result<(), gnata::JsonataError> {
    let expr = Expression::compile("$sum(Order.Product.(Price * Quantity))")?;
    let result = expr.evaluate(r#"{
        "Order": [
            {"Product": [{"Price": 34.5, "Quantity": 2}]},
            {"Product": [{"Price": 21.5, "Quantity": 1}]}
        ]
    }"#)?;
    println!("{}", result.stringify(false)?); // 90.5
    Ok(())
}
```

A compiled `Expression` is `Send + Sync` and cheap to `Clone` (the AST is
shared), so compile once and evaluate many times — including from multiple
threads. If you already have a parsed [`Value`], use `evaluate_value` to
skip JSON parsing.

## Custom functions

```rust
use std::sync::Arc;
use gnata::{CustomFunc, Expression, Value, new_custom_env};

let double: CustomFunc = Arc::new(|args, _focus| {
    let n = args.first().and_then(Value::as_f64).unwrap_or(0.0);
    Ok(Value::Number(n * 2.0))
});

let env = new_custom_env(&[("double".into(), double)]);
let expr = Expression::compile("$double(21)")?;
let result = expr.evaluate_with_env(&Value::Undefined, &env)?;
```

See [`examples/`](examples/) for variables, error handling, streaming
evaluation, and more.

## Cargo features

| Feature | Default | Purpose |
|---|---|---|
| `regex` | yes | Full Unicode regex backend (RE2 semantics, no backtracking) |
| `regex-lite` | no | Lighter regex backend; shrinks WASM builds by ~250 KB |
| `mimalloc-alloc` | yes | mimalloc as the global allocator on native targets |

Exactly one regex backend must be enabled. To use `regex-lite`, disable
default features and re-enable what you need:

```toml
gnata = { version = "0.1", default-features = false, features = ["regex-lite", "mimalloc-alloc"] }
```

## WebAssembly

The crate builds for `wasm32-unknown-unknown` and ships `wasm-bindgen`
bindings (compile, evaluate, format, highlight) in its `wasm` module.
Combined with `regex-lite` and `wasm-opt`, the optimized module is a small
fraction of the size of the equivalent Go/TinyGo build.

## Performance

Compiled natively, this engine outperforms the Go implementation on all 32
benchmarks in the repository's cross-language suite (median 1.6x, up to
2.4x), and the reference JavaScript implementation by a wide margin. The
WASM build also beats native Go on the same suite. See
`bench/benchmark_results.csv` in the repository for the full data.

## Semantics guarantees

- `undefined` and `null` are distinct: `undefined = undefined` is `false`,
  `null = null` is `true`
- Object key insertion order is preserved through all operations
- Number formatting matches JavaScript's `Number.toString()`
- Tail calls are trampolined — deep recursion cannot overflow the stack
- Regex uses finite-automaton semantics (no catastrophic backtracking)

## Minimum supported Rust version

Rust 1.88 (edition 2024 plus let-chains).

## License

MIT. See [LICENSE](LICENSE).
