# Baseline Measurements (2026-04-05)

Captured before CompactString, Rc<[Value]>, and arena optimizations.
Commit: `init-rs` branch, post-halfbrown + BinaryOp enum.

## Hyperfine: Go vs Rust (process-level, includes startup + parse)

### 10k Short (2.0 MB, 100 iters)
| Benchmark    | Go µs/op | Rust µs/op | Ratio |
|-------------|----------|------------|-------|
| nested_path | 1052     | 777        | 0.74x |
| filter_path | 1949     | 1929       | 0.99x |
| aggregation | 4865     | 3518       | 0.72x |
| func_count  | 1120     | 865        | 0.77x |

### 10k Long (5.6 MB, 100 iters)
| Benchmark    | Go µs/op | Rust µs/op | Ratio |
|-------------|----------|------------|-------|
| nested_path | 1247     | 992        | 0.80x |
| filter_path | 2178     | 1817       | 0.83x |
| aggregation | 4740     | 3731       | 0.79x |
| func_count  | 1362     | 1091       | 0.80x |

### 10k Mixed (2.9 MB, 100 iters)
| Benchmark    | Go µs/op | Rust µs/op | Ratio |
|-------------|----------|------------|-------|
| nested_path | 1122     | 950        | 0.85x |
| filter_path | 2031     | 1707       | 0.84x |
| aggregation | 4451     | 3329       | 0.75x |
| func_count  | 1130     | 952        | 0.84x |

### 100k Short (20.3 MB, 10 iters)
| Benchmark    | Go µs/op  | Rust µs/op | Ratio |
|-------------|-----------|------------|-------|
| nested_path | 42,360    | 38,650     | 0.91x |
| filter_path | 50,610    | 50,300     | 0.99x |
| aggregation | 74,730    | 61,650     | 0.82x |
| func_count  | 40,360    | 36,320     | 0.90x |

### 100k Long (56.3 MB, 10 iters)
| Benchmark    | Go µs/op  | Rust µs/op | Ratio |
|-------------|-----------|------------|-------|
| nested_path | 67,430    | 56,180     | 0.83x |
| filter_path | 70,740    | 63,070     | 0.89x |
| aggregation | 94,270    | 75,120     | 0.80x |
| func_count  | 62,010    | 52,860     | 0.85x |

### 100k Mixed (29.0 MB, 10 iters)
| Benchmark    | Go µs/op  | Rust µs/op | Ratio |
|-------------|-----------|------------|-------|
| nested_path | 47,350    | 50,090     | **1.06x** |
| filter_path | 54,830    | 58,310     | **1.06x** |
| aggregation | 77,280    | 69,060     | 0.89x |
| func_count  | 46,200    | 48,460     | **1.05x** |

**Key finding**: At 100k mixed, Go wins on path/filter/count by 5-6%. 
Aggregation (compute-heavy) still favors Rust. This indicates string 
allocation overhead in path traversal is the bottleneck.

## Memory

| Tag          |   Bytes | Go RSS(KB) | Rust RSS(KB) | Go TotalAlloc | DHAT Total  | DHAT Blocks |
|-------------|---------|-----------|-------------|--------------|------------|------------|
| tiny_short  |     477 |     8,684 |       4,080 |   17,042,544 |   6,434,443 |    120,348 |
| 1k_short    | 199,723 |     9,588 |       5,652 |   19,959,232 |  22,534,205 |     24,567 |
| 10k_short   |2,006,036|    25,452 |      24,472 |   40,908,216 |  57,925,641 |    204,137 |
| 10k_long    |5,615,891|    33,516 |      34,648 |   49,300,392 |  71,050,558 |    233,156 |
| 10k_mixed   |2,886,391|    27,688 |      27,456 |   37,948,536 |  58,386,757 |    233,157 |
| 100k_short  |20,253,657|  143,656 |     210,528 |  195,930,272 | 420,982,037 |  2,002,313 |
| 100k_long   |56,259,108|  218,920 |     313,820 |  341,742,840 | 606,768,826 |  2,096,080 |
| 100k_mixed  |28,964,153|  176,552 |     240,416 |  227,518,304 | 479,874,421 |  2,096,083 |

**Key finding**: Rust peak RSS is 1.36-1.47x Go at 100k scale. DHAT total 
alloc is 1.78-2.15x Go TotalAlloc. Main driver: Rc wrapping overhead + 
no intermediate GC reclamation.

## Files

- `hyperfine_short.txt` / `hyperfine_long.txt` / `hyperfine_mixed.txt` — full output
- `result_*.json` — hyperfine JSON (machine-readable)
- `criterion_eval.txt` / `criterion_parse.txt` — Rust-internal benchmarks
- `memory_baseline.txt` — memory profiling output
