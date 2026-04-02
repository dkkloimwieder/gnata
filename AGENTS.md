# AGENTS.md

This file provides guidance to AI agents (Claude Code, GitHub Copilot, Cursor, etc.) when working with code in this repository.

## What Is Gnata

Gnata is a full JSONata 2.x implementation in Go, built for production streaming workloads. JSONata is a query and transformation language for JSON data. Gnata provides two-tier evaluation: fast-path (GJSON zero-copy) for simple expressions and full AST evaluation for complex ones, plus a lock-free `StreamEvaluator` for high-throughput batched evaluation.

## Development Commands

```sh
# Lint (from package root)
golangci-lint run

# Run all tests (1,273 JSON test cases from official jsonata-js suite + unit tests)
go test ./...

# Run a single test
go test -run TestName

# Run benchmarks
go test -bench=. -benchmem
```

## Architecture

### Compilation Pipeline

Lexer → Parser → AST Processing → Fast-Path Analysis → Expression

1. **Lexer** (`internal/lexer/`) — Tokenizes JSONata expression strings
2. **Parser** (`internal/parser/`) — Pratt (top-down operator precedence) parser producing AST nodes
3. **AST Processing** (`parser.ProcessAST`) — Normalizes and optimizes the AST
4. **Fast-Path Analysis** (`parser.AnalyzeFastPath`) — Classifies expressions into:
   - Pure-path fast path (e.g., `Account.Name`) — uses GJSON zero-copy
   - Comparison fast path (e.g., `a.b = "x"`) — zero allocations
   - Full AST evaluation required

### Two-Tier Evaluation

- `Eval(ctx, any)` — Evaluate against pre-parsed Go values via full AST walk
- `EvalBytes(ctx, json.RawMessage)` — Fast-path expressions use GJSON directly on raw JSON bytes; full-path falls back to unmarshal + Eval

### StreamEvaluator (stream.go)

Batch-evaluates multiple expressions against events. Schema-keyed `GroupPlan` caching deduplicates field extraction across expressions. Lock-free reads via `atomic.Pointer` snapshot; writes serialized by `sync.Mutex`. Single JSON scan per event via `gjson.GetManyBytes`.

### Evaluator Dispatch (internal/evaluator/)

`evaluator.Eval(node, input, env)` dispatches by `node.Type`. Each eval category is in its own file:
- `eval_binary.go` — Binary operators, subscripts, filtering
- `eval_function.go` — Function calls, lambdas, partial application
- `eval_chain.go` — Path chaining, pipes, blocks, conditions
- `eval_sort.go` — Sorting with generic `SortItemsErr[T any]` helper
- `eval_transform.go` — JSONata transform operator (`|obj|updates|deletes|` syntax)
- `eval_group.go` — Group-by reduction (`{key: val}` syntax)
- `eval_range.go` — Range operator (`[start..end]`)
- `eval_regex.go` — Regex compilation and matching
- `eval_unary.go` — Unary operators (negation, array constructor)

### Standard Library (functions/)

55+ built-in JSONata functions across categorized files: `string_funcs.go`, `string_match_replace.go`, `string_format_number.go`, `string_format_integer.go`, `string_encoding.go`, `numeric_funcs.go`, `array_funcs.go`, `object_funcs.go`, `hof_funcs.go`, `boolean_funcs.go`, `datetime_funcs.go`, `datetime_format.go`, `datetime_parse.go`. All registered via `functions.RegisterAll`.

### Fast-Path Byte Evaluation (func_fast.go)

Dispatch-map of `funcFastHandlers` maps each `FuncFastKind` to a standalone handler function (e.g., `evalFuncContains`, `evalFuncString`). Each handler operates directly on `gjson.Result` for zero-copy evaluation. `FuncFastRound` is intentionally absent — it requires banker's rounding handled by the full evaluator.

### Key Types

- **Expression** (`gnata.go`) — Compiled, goroutine-safe JSONata expression with fast-path metadata
- **StreamEvaluator** (`stream.go`) — Copy-on-write expression slice + `BoundedCache` for schema plans
- **BoundedCache** (`bounded_cache.go`) — Lock-free FIFO ring-buffer cache (atomic pointer reads)
- **OrderedMap** (`internal/evaluator/ordered_map.go`) — Insertion-ordered map preserving JSON field order
- **Environment** (`internal/evaluator/env.go`) — Lexical scope chain for variable bindings

## Dependencies

Only one direct dependency (pure Go, no CGo):
- `tidwall/gjson` — Zero-copy JSON field extraction for fast-path byte-level evaluation

Regex uses Go's standard `regexp` package (no external regex library).

## Testing

Tests use separate `_test` packages (`gnata_test`, `lexer_test`, `parser_test`). The primary test suite (`suite_test.go`) loads 1,200+ JSON test cases from `testdata/groups/` (100+ subdirectories) — each `.json` file is a case with `expr`, `dataset`, `bindings`, and `result` fields. Datasets live in `testdata/datasets/`. Additional unit tests in `evaluator_test.go` cover regression tests using table-driven test (TDT) style.

## Custom Functions

Register custom functions via `StreamEvaluator` options or standalone `CustomEnv`:
```go
customFuncs := map[string]gnata.CustomFunc{
    "md5": func(args []any, focus any) (any, error) { ... },
}
se := gnata.NewStreamEvaluator(nil, gnata.WithCustomFunctions(customFuncs))
```

## WASM

`wasm/main.go` exports `gnataEval`, `gnataCompile`, `gnataEvalHandle` for browser use. Build with `GOOS=js GOARCH=wasm go build -ldflags="-s -w" -trimpath -o gnata.wasm ./wasm/`.

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
