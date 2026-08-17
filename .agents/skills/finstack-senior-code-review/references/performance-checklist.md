# Performance Checklist

Language-specific performance patterns and anti-patterns. Only optimize what's on the hot path — but when it's on the hot path, optimize it properly.

## Universal Principles

1. **Measure first.** No optimization without a benchmark or profile.
2. **Algorithmic complexity beats micro-optimization.** Fix the O(n²) before worrying about cache lines.
3. **I/O dominates compute.** Most "slow" code is waiting on disk, network, or database.
4. **Allocation is not free.** Heap allocations in tight loops are the #1 performance killer across all languages.
5. **Batch everything.** One query returning 1000 rows beats 1000 queries returning 1 row.

## Rust

### Hot Path Patterns

```rust
// BAD: allocation in loop
for item in data {
    let result = format!("processed: {}", item);  // allocates every iteration
    results.push(result);
}

// GOOD: pre-allocate
let mut results = Vec::with_capacity(data.len());
for item in data {
    results.push(item.process());  // no format!, compute in place
}
```

```rust
// BAD: unnecessary clone
fn process(data: &[Record]) -> Vec<Record> {
    data.iter().filter(|r| r.active).cloned().collect()
}

// GOOD: return references or indices when possible
fn active_indices(data: &[Record]) -> Vec<usize> {
    data.iter().enumerate()
        .filter(|(_, r)| r.active)
        .map(|(i, _)| i)
        .collect()
}
```

### Memory Layout

```rust
// BAD: array of structs for columnar access
struct Sample { timestamp: i64, value: f64, weight: f64 }
let samples: Vec<Sample> = ...;  // cache-unfriendly if only accessing values

// GOOD: struct of arrays when accessing one field at a time
struct SampleData {
    timestamps: Vec<i64>,
    values: Vec<f64>,
    weights: Vec<f64>,
}
```

### Concurrency

- Use `rayon` for CPU-bound parallelism on collections — it's nearly free.
- Use `tokio` for I/O-bound work. Don't mix blocking and async carelessly.
- `Arc<Mutex<T>>` is fine for low-contention shared state. Don't reach for lock-free structures unless contention is measured.

### Common Rust Pitfalls

- `.collect()` into a HashMap when you only need to iterate — just use an iterator.
- `String` where `&str` suffices. Own data only when you need to.
- `Box<dyn Trait>` when an enum with 2-3 variants would be faster (no vtable dispatch).

## Python

### Vectorization

```python
# BAD: Python loop over numeric data
result = []
for i in range(len(values)):
    result.append(values[i] * weights[i])

# GOOD: NumPy vectorized
result = values * weights
```

```python
# BAD: iterating a DataFrame row by row
for idx, row in df.iterrows():
    df.loc[idx, "total"] = row["count"] * row["weight"]

# GOOD: vectorized column operation
df["total"] = df["count"] * df["weight"]
```

### Memory

```python
# BAD: loading entire dataset when you need a slice
df = pd.read_csv("massive_file.csv")  # 10GB in memory
result = df[df["date"] == today]

# GOOD: filter during read or use chunked processing
df = pd.read_csv("massive_file.csv", usecols=["date", "value", "weight"], dtype={"value": "float32", "weight": "int32"})
```

### Avoid These

- `pandas.apply()` with a Python function — it's just a loop in disguise. Use vectorized ops or `np.where`.
- `copy.deepcopy()` in hot paths. Restructure to avoid needing it.
- f-string formatting in tight loops when you're just building keys — use tuples as dict keys instead.
- Class instantiation in inner loops when a named tuple or dataclass would do (or just a tuple).
- Global imports of heavy modules at function scope — import at module level.

### Concurrency

- `multiprocessing` for CPU-bound work (GIL bypass). Use `Pool.map` for embarrassingly parallel tasks.
- `asyncio` for I/O-bound work (API calls, database queries).
- `threading` is almost never what you want in CPython for compute. It's fine for I/O.
- `concurrent.futures.ProcessPoolExecutor` is the clean API over multiprocessing.

## WASM/JS

### Numerical Precision

```javascript
// BAD: IEEE 754 surprise
0.1 + 0.2  // 0.30000000000000004

// GOOD: use integer arithmetic when exact sums matter
const totalCents = 10 + 20;  // 30
// Or use a decimal library when the domain requires exact decimal math
```

### WASM Interop

- Minimize data crossing the WASM boundary — each crossing has overhead.
- Pass typed arrays (`Float64Array`) instead of JavaScript arrays.
- Do bulk computation in WASM, return results in one call. Don't call into WASM per-element.

### Bundle Size

- Tree-shake aggressively. Don't import lodash for `_.get()`.
- Lazy-load modules that aren't needed at startup.
- No framework if vanilla JS does the job (dashboards, internal tools).

## SQL

### Query Performance

```sql
-- BAD: correlated subquery
SELECT t.*,
    (SELECT MAX(value) FROM samples s WHERE s.key = t.key AND s.ts <= t.ts)
FROM events t;

-- GOOD: window function
SELECT t.*,
    MAX(value) OVER (PARTITION BY key ORDER BY ts ROWS UNBOUNDED PRECEDING)
FROM events t
JOIN samples s ON t.key = s.key AND t.ts = s.ts;
```

```sql
-- BAD: SELECT * when you need 3 columns
SELECT * FROM sessions WHERE date = CURRENT_DATE;

-- GOOD: select only what you need
SELECT id, status, updated_at FROM sessions WHERE date = CURRENT_DATE;
```

### Index Awareness

- Filter columns should have indexes. If you WHERE on it, index it.
- Composite indexes: put equality filters first, range filters last.
- Don't index columns with low cardinality (boolean flags) unless combined with high-cardinality columns.
- Check EXPLAIN plans for sequential scans on large tables.

### Common SQL Pitfalls

- `DISTINCT` as a band-aid for bad joins — fix the join, don't deduplicate.
- `ORDER BY` on non-indexed columns in large result sets.
- `NOT IN (subquery)` with NULLs — use `NOT EXISTS` instead.
- Implicit type casting in WHERE clauses that prevent index usage.
- N+1 query patterns from ORM lazy loading.
