# CLAUDE.md

This file provides authoritative guidance for Claude Code sessions working on the gnata Rust migration.

## Project Identity

**gnata-rs** is a Rust port of gnata, a full JSONata 2.x query and transformation language engine originally written in Go (~16K lines). The Go reference implementation lives alongside the Rust code during migration.

- **Branch**: `init-rs`
- **Goal**: Feature-complete Rust implementation passing all 1,349 test cases (1,283 conformance + 66 supplemental)
- **Reference**: Go source in repo root (`*.go`) and `internal/`, `functions/`

## Architecture Principles -- Memory Model (Path B Hybrid)

**AST**: Index-based arena (`AstArena` with `Vec<Expr>` and `NodeId(u32)`). No `Box<Node>`, `Rc`, or lifetime annotations. `Send + Sync`. Parser takes `&mut AstArena`, returns `NodeId`. Evaluator takes `&AstArena`.

**Values**: The `Value` enum uses standard owned types (`String`, `Vec`, `IndexMap`). It explicitly **DOES NOT** use lifetimes (`<'bump>`), ensuring the public API is clean and return values are fully owned.

**Sequences**: Implement a dedicated `Sequence` struct (wrapped in `Value::Sequence`) to handle `KeepSingleton` and `ConsArray` flags, distinct from standard `Value::Array`. This is an internal-only variant -- never exposed to users.

**State**: Use `bumpalo::Bump` internally per-evaluation **strictly** for the `Environment` chain and intermediate state to ensure fast O(1) cleanup. NOT for AST, NOT for return values.

**Concurrency**: `ArcSwap` for lock-free COW snapshots of the AST and execution plans. `DashMap` for regex caches -- **must clone the `Arc` before execution to drop the shard lock**.

**Type Fidelity**: `Undefined != Null` semantics must be preserved via `Value::Undefined` and `Value::Null`.

**Error Handling**: `Result<Value, JsonataError>` everywhere. No panics. Use `thiserror` for JSONata spec error codes.

**Execution**: TCO must be implemented via a **Trampoline loop** returning a `TailCall` state, preventing stack overflows on deep recursion.

## Development Commands

```sh
# Go (reference implementation)
go test ./...                  # Run all Go tests (1,283 conformance + unit tests)
go test -run TestName          # Run specific Go test
go test -bench=. -benchmem     # Benchmarks
golangci-lint run              # Lint Go code

# Rust (port)
cargo build                    # Build
cargo test                     # Run all tests
cargo test -- --nocapture      # Tests with stdout
cargo clippy                   # Lint
cargo bench                    # Benchmarks
cargo build --target wasm32-unknown-unknown  # WASM build
```

## Testing Strategy

- Port the conformance test harness from `suite_test.go` to load JSON test cases from `testdata/groups/`
- Same test case format: `expr`, `data`/`dataset`, `result`/`undefinedResult`/`code`
- Run Go tests (`go test ./...`) to validate any new test cases added
- Track conformance progress: X/1349 passing

## Key Dependencies & Why

| Crate | Purpose | Replaces |
|---|---|---|
| `arc-swap` | Lock-free COW snapshots | Go `atomic.Pointer` |
| `dashmap` | Concurrent HashMap | Go `sync.Map` |
| `parking_lot` | Faster Mutex/RwLock | Go `sync.Mutex` |
| `bumpalo` | Per-eval bump allocator for environments | Go GC |
| `regex` | RE2-semantics regex | Go `regexp` |
| `serde_json` + `arbitrary_precision` + `preserve_order` | JSON with precision + ordered maps | Go `encoding/json` + `OrderedMap` |
| `indexmap` | Insertion-ordered maps | Go `OrderedMap` |
| `ryu-js` | ECMAScript `Number.toString()` formatting | Go `FormatFloat` |
| `jiff` | Timezone-aware datetime | Go `time` |
| `thiserror` | Structured error types | Go `JSONataError` |
| `base64` | Base64 encode/decode | Go `encoding/base64` |
| `percent-encoding` | URL encoding | Go `net/url` |
| `unicode-segmentation` | Grapheme cluster awareness | Go `unicode` |
| `fastrand` | Fast non-crypto RNG | Go `math/rand` |
| `wasm-bindgen` | WASM JS interop (wasm32 only) | Go `syscall/js` |

## Behavioral Invariants (DO NOT VIOLATE)

1. `undefined = undefined` returns `false` (not `true`)
2. `null = null` returns `true`
3. Sequence collapse: len 0 -> None, len 1 -> unwrap (unless KeepSingleton), len > 1 -> array
4. Auto-mapping: field access on arrays maps across elements and flattens
5. `$eval()` shares call counter with parent evaluation
6. Tail-call optimization via trampoline (max iterations = depth * 10000)
7. Sort is stable; multi-key comparison; nil sorts after non-nil
8. Object key insertion order must be preserved throughout all operations
9. Number formatting must match JavaScript's `Number.toString()` (use `ryu-js`)
10. Boolean coercion: `"0"` is truthy, `""` is falsy, `"false"` is truthy

## Go Compilation Pipeline (Reference)

```
Expression String -> Lexer -> Parser -> ProcessAST -> AnalyzeFastPath -> Expression
```

1. **Lexer** (`internal/lexer/`) -- 54 token types, context-sensitive `/`
2. **Parser** (`internal/parser/`) -- Pratt parser with binding power table
3. **AST Processing** (`parser.ProcessAST`) -- flattens `.` chains into paths, marks tail calls
4. **Fast-Path Analysis** (`parser.AnalyzeFastPath`) -- classifies for GJSON optimization (defer in Rust)
5. **Evaluation** (`internal/evaluator/`) -- tree-walking dispatch by node type

## Key Design Decisions

- **Parser**: Hand-written Pratt parser, directly translated from Go. No parser combinator libraries.
- **Regex**: `regex` crate -- same RE2/finite-automaton semantics as Go's `regexp`. Named captures, flags, no backtracking.
- **Concurrency**: `arc-swap` -- direct translation of Go's COW pattern. No architectural redesign.
- **Fast-Path**: Deferred until after full evaluator passes all tests. Performance feature, not correctness.
- **WASM**: 50-200 KB binaries (vs 2-3 MB Go). `wasm-bindgen` + `wasm-pack` + `wasm-opt`.
- **Number formatting**: `ryu-js` for exact ECMAScript `Number.toString()`. Custom helper for round-half-away-from-zero (Rust's `f64::round()` uses banker's rounding).
- **Cancellation**: `Arc<AtomicBool>` checked at expression boundaries. No async needed.

See `docs/rust-migration-plan.md` for full rationale, code examples, and the 13-phase implementation sequence.

## Reference

- **Migration plan**: `docs/rust-migration-plan.md` -- dependencies, architecture, implementation phases
- **Behavioral spec**: `docs/spec.md` -- 1,966-line authoritative reference for all JSONata semantics
- **Migration hazards**: `docs/migration-hazards.md` -- 10 ranked Go→Rust pitfalls with code examples
- **Behaviors catalog**: `docs/behaviors.md` -- truth tables, error codes, equality rules
- **Go source**: `*.go`, `internal/`, `functions/`
- **Test suite**: `testdata/groups/` (113 directories, 1,349 cases)
- **Test datasets**: `testdata/datasets/`
- **JSONata spec**: https://jsonata.org
