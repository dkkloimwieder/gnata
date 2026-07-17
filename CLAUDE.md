# CLAUDE.md

Guidance for Claude Code sessions in this repo.

## What this is

**gnata** is a JSONata 2.x query and transformation engine. The production
implementation is the Rust crate in `crates/gnata-rs` (crate name `gnata`);
the original Go implementation (repo root `*.go`, `internal/`, `functions/`)
is kept as the behavioral reference. The port is feature-complete: all 1,733
conformance cases pass, and `tests/conformance.rs` asserts zero failures.

## Commands (run in `crates/gnata-rs`)

```sh
cargo test                             # all tests; conformance gate is strict
cargo clippy --release --all-targets   # gate: 121 warnings, 0 errors — do not add any
cargo fmt --check                      # must stay clean
cargo bench                            # criterion benches
cargo check --target wasm32-unknown-unknown --no-default-features --features regex-lite
../../scripts/build-wasm.sh            # optimized WASM build (run from repo root)
go test ./...                          # Go reference suite (repo root)
```

## Architecture (as built)

- **AST**: index-based arena (`AstArena` = `Vec<Expr>` + `NodeId(u32)`), built
  by a hand-written Pratt parser; `process_ast` flattens `.` chains into paths
  and marks tail calls. `Expression` wraps the arena in an `Arc`: `Send + Sync`,
  cheap to clone — compile once, evaluate from any thread.
- **`Value`**: 16-byte enum; the size is load-bearing (Rc-wrapping experiments
  that grew it regressed benchmarks 12–40%). Strings are `CompactString` (inline
  ≤24 bytes; beat `Rc<str>` on cache locality in A/B tests), arrays/objects are
  `Rc<[Value]>` / `Rc<ObjectMap>` with copy-on-write via `Rc::make_mut`,
  functions are `Box<FunctionValue>`. Deliberately `!Send`: share the
  `Expression`, build input `Value`s per thread.
- **`Sequence`**: internal-only `Value` variant carrying the
  `keep_singleton`/`cons_array` flags. `eval()` collapses it at the API
  boundary; it must never reach users.
- **`Environment`**: `Rc` parent chain with `RefCell` bindings and a small
  cache for non-local lookups; carries the shared call counter and an
  `Arc<AtomicBool>` cancellation flag.
- **Errors**: hand-rolled `JsonataError { code, token, value, message }` with
  JSONata spec codes; `Result` everywhere. Panics only for documented caller
  bugs (e.g. foreign `NodeId` in `AstArena::get`).
- **Recursion**: tail calls run through a trampoline over a `TailCall` value
  (max iterations = max call depth × 10,000); other deep recursion is covered
  by `stacker::maybe_grow` on native targets (wasm cannot grow its stack).
- **Fast paths**: `fast_path.rs` (simple path/tape evaluation) and
  `stdlib/hof_fast.rs` (lambda recognition for HOFs) bypass general dispatch.
  They must be semantics-preserving: `tests/differential.rs` and the fuzz
  targets compare them against the general path. When a pattern is ambiguous,
  don't lift it.
- **JSON**: simd-json parses input; serde_json (`arbitrary_precision` +
  `preserve_order`) serializes output; `ryu-js` gives exact ECMAScript
  `Number.toString()`. indexmap keeps object key order.
- **Datetime / encodings**: hand-rolled calendar math (no jiff/chrono);
  base64 and percent-encoding crates for the encoding builtins.
- **Regex**: `regex` (default) or `regex-lite` (small WASM builds) — enable
  exactly one; both are finite-automaton, no backtracking.
- **Public API**: the curated re-exports in `lib.rs`; all modules are
  `pub(crate)`. The `#[doc(hidden)]` re-exports exist for in-repo
  tests/benches/fuzz only and carry no stability guarantee.

## Behavioral invariants (DO NOT VIOLATE)

1. `undefined = undefined` → `false`; `null = null` → `true`
2. Sequence collapse: 0 items → undefined, 1 → unwrapped (unless
   keep-singleton), >1 → array
3. Field access on arrays auto-maps and flattens
4. Object key insertion order is preserved through all operations
5. Number output matches JS `Number.toString()`; `$round` is half-to-even
6. Boolean coercion: `"0"` truthy, `""` falsy, `"false"` truthy
7. Sort is stable; nils sort after non-nils
8. `$eval()` shares the call counter with its parent evaluation

## Conventions

- Lint suppressions use `#[expect(...)]`, with a `reason` when non-obvious;
  tests may unwrap/panic (see `clippy.toml`). Keep `cargo fmt --check` clean.
- Gate performance changes with a same-session A/B (saved criterion baselines
  drift); gate correctness changes on the conformance suite.

## References

- `docs/spec.md` — authoritative behavioral spec (derived from the Go code)
- `docs/behaviors.md` — truth tables, error codes, equality rules
- `docs/migration-hazards.md` — Go→Rust pitfalls encountered in the port
- `docs/rust-migration-plan.md` — **historical**: the original plan; several
  decisions changed during implementation. Where it disagrees with the code
  or this file, the code wins.
- Test suite: `testdata/groups/` (112 groups) + `testdata/datasets/`
- JSONata language: https://jsonata.org
