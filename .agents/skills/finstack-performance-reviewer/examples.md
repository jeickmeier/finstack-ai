# Performance Review Examples

## Example 1: Algorithmic complexity issue

### Code under review

```rust
fn find_duplicates(items: &[String]) -> Vec<String> {
    let mut duplicates = Vec::new();
    for i in 0..items.len() {
        for j in (i + 1)..items.len() {
            if items[i] == items[j] && !duplicates.contains(&items[i]) {
                duplicates.push(items[i].clone());
            }
        }
    }
    duplicates
}
```

### Review output

## Summary
- Function to find duplicate strings in a slice
- **High** performance risk: O(n³) complexity

## Performance concerns
- Nested loops create O(n²) comparisons
- `duplicates.contains()` adds another O(n) factor, making total O(n³)
- String cloning in hot path adds allocation overhead
- Will become unusable beyond ~1000 items

## Findings

### Blockers
- **O(n³) complexity**: For 10K items, this is ~10¹² operations. Fix by using HashSet for both seen tracking and duplicate detection. Expected: O(n) with hash operations.

### Majors
- **Unnecessary string cloning**: Cloning on every duplicate found. Consider returning indices or references if caller doesn't need owned strings.

## Action items
- [ ] Replace with HashSet-based O(n) implementation
- [ ] Add benchmark comparing before/after at various sizes
- [ ] Consider returning `Vec<&str>` if ownership not needed

### Fixed implementation

```rust
use std::collections::HashSet;

fn find_duplicates(items: &[String]) -> Vec<String> {
    let mut seen = HashSet::with_capacity(items.len());
    let mut duplicates = HashSet::new();

    for item in items {
        if !seen.insert(item) {
            duplicates.insert(item.clone());
        }
    }

    duplicates.into_iter().collect()
}
```

---

## Example 2: Allocation in hot path

### Code under review

```rust
fn lookup_session_scores(
    sessions: &[Session],
    scores: &HashMap<String, f64>,
) -> Vec<f64> {
    sessions
        .iter()
        .map(|session| {
            let key = format!("{}:{}", session.tenant, session.id);
            *scores.get(&key).unwrap_or(&0.0)
        })
        .collect()
}
```

### Review output

## Summary
- Session score lookup called frequently
- **Major** performance risk: allocation per session

## Performance concerns
- `format!()` allocates a new String for every session
- With 10K sessions at 60Hz, that's 600K allocations/second
- Key computation is repeated; should be cached or restructured

## Findings

### Majors
- **String allocation in hot loop**: `format!()` creates heap allocation per iteration. If called frequently with same positions, consider pre-computing keys or restructuring the price lookup.

### Minors
- **HashMap key type**: If this is a hot path, consider using a numeric ID instead of string keys, or use `FxHashMap` with a composite key struct.

## Benchmarking recommendations
- Measure with realistic session counts (100, 1K, 10K)
- Profile allocation rate with DHAT or similar

## Action items
- [ ] Pre-compute keys in Session struct or use numeric IDs
- [ ] If keys must be computed, use a reusable buffer
- [ ] Consider FxHashMap if string keys are necessary

### Fixed implementation

```rust
// Option A: Pre-computed key
struct Session {
    tenant: String,
    id: String,
    score_key: String,  // Pre-computed: "{tenant}:{id}"
}

// Option B: Numeric lookup
fn lookup_session_scores(
    sessions: &[Session],
    scores: &[f64],  // Indexed by session_id
) -> Vec<f64> {
    sessions
        .iter()
        .map(|session| scores[session.score_index])
        .collect()
}

// Option C: Reusable buffer (if restructuring not possible)
fn lookup_session_scores(
    sessions: &[Session],
    scores: &HashMap<String, f64>,
) -> Vec<f64> {
    let mut key_buf = String::with_capacity(64);
    sessions
        .iter()
        .map(|session| {
            key_buf.clear();
            key_buf.push_str(&session.tenant);
            key_buf.push(':');
            key_buf.push_str(&session.id);
            *scores.get(&key_buf).unwrap_or(&0.0)
        })
        .collect()
}
```

---

## Example 3: Cache-hostile data access

### Code under review

```rust
struct Record {
    id: u64,
    name: String,
    kind: String,
    payload: Box<Payload>,  // 200+ bytes
    metadata: Box<Metadata>,  // 500+ bytes
    // ... many more fields
}

fn sum_payload_sizes(records: &[Record]) -> usize {
    records.iter().map(|r| r.payload.size).sum()
}
```

### Review output

## Summary
- Simple aggregation over record collection
- **Major** performance risk: cache-hostile memory layout

## Performance concerns
- Each `Record` is large; iterating loads entire struct into cache
- `payload` is behind a Box, causing pointer chase per record
- Only accessing `size` but loading hundreds of bytes per item
- Poor cache utilization will dominate for large collections

## Findings

### Majors
- **Pointer chasing**: `Box<Payload>` requires dereferencing a pointer for each record, destroying spatial locality. For hot-path aggregations, consider storing frequently-accessed fields directly or using SoA layout.
- **Cold data in hot path**: Large struct with many unused fields loaded into cache. Consider separating hot fields (size, id) from cold fields (name, metadata).

## Action items
- [ ] Store `size` directly in `Record` if accessed frequently
- [ ] Consider SoA layout for hot-path aggregations
- [ ] Profile with `perf stat` to measure cache misses

### Fixed implementation

```rust
// Option A: Inline hot field
struct Record {
    id: u64,
    size: usize,  // Moved from Payload to avoid indirection
    name: String,
    kind: String,
    payload: Box<Payload>,
    metadata: Box<Metadata>,
}

// Option B: Struct of Arrays for hot-path operations
struct RecordStore {
    // Hot data (used in aggregations)
    ids: Vec<u64>,
    sizes: Vec<usize>,

    // Cold data (used occasionally)
    names: Vec<String>,
    kinds: Vec<String>,
    payloads: Vec<Payload>,
    metadata: Vec<Metadata>,
}

impl RecordStore {
    fn sum_payload_sizes(&self) -> usize {
        self.sizes.iter().sum()  // Cache-friendly sequential access
    }
}
```

---

## Example 4: Missing parallelization

### Code under review

```rust
fn apply_records(
    records: &[Record],
    context: &ApplyContext,
) -> Vec<ApplyResult> {
    records
        .iter()
        .map(|record| apply_single(record, context))
        .collect()
}

fn apply_single(record: &Record, context: &ApplyContext) -> ApplyResult {
    // Complex apply logic, ~1ms per record
    // ...
}
```

### Review output

## Summary
- Applies a collection of records sequentially
- **Major** performance opportunity: embarrassingly parallel workload

## Performance concerns
- Each apply is independent (~1ms each)
- 1000 records = 1 second sequential, could be ~125ms on 8 cores
- No shared mutable state prevents parallelization

## Findings

### Majors
- **Missing parallelization**: Independent apply operations with significant per-item cost. This is an ideal candidate for `rayon::par_iter()`.

### Minors
- Ensure `ApplyContext` is `Sync` (read-only access is safe to share).

## Benchmarking recommendations
- Measure single-record apply time to confirm parallelization is worthwhile (>1μs)
- Test with realistic record counts

## Action items
- [ ] Add rayon dependency if not present
- [ ] Replace `iter()` with `par_iter()`
- [ ] Verify `ApplyContext` is thread-safe for concurrent reads

### Fixed implementation

```rust
use rayon::prelude::*;

fn apply_records(
    records: &[Record],
    context: &ApplyContext,  // Must be Sync
) -> Vec<ApplyResult> {
    records
        .par_iter()  // Parallel iteration
        .map(|record| apply_single(record, context))
        .collect()
}
```

---

## Example 5: Over-optimization (anti-pattern)

### Code under review

```rust
/// Sums values using SIMD intrinsics for maximum performance
#[cfg(target_arch = "x86_64")]
pub unsafe fn simd_sum(values: &[f64]) -> f64 {
    use std::arch::x86_64::*;

    let mut sum = _mm256_setzero_pd();
    let chunks = values.chunks_exact(4);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let v = _mm256_loadu_pd(chunk.as_ptr());
        sum = _mm256_add_pd(sum, v);
    }

    // Horizontal sum
    let low = _mm256_castpd256_pd128(sum);
    let high = _mm256_extractf128_pd(sum, 1);
    let sum128 = _mm_add_pd(low, high);
    let high64 = _mm_unpackhi_pd(sum128, sum128);
    let result = _mm_add_sd(sum128, high64);

    let mut total = _mm_cvtsd_f64(result);
    for &v in remainder {
        total += v;
    }
    total
}
```

### Review output

## Summary
- Manual SIMD implementation for summing f64 values
- **Nit**: Over-engineered; likely no real-world benefit

## Performance concerns
- Manual SIMD adds significant complexity and maintenance burden
- Requires `unsafe` with no safety documentation
- Platform-specific (x86_64 only)
- Standard library iterator `.sum()` likely auto-vectorizes to equivalent code
- No benchmark proving this is faster than simple implementation

## Findings

### Minors
- **Premature optimization**: Without benchmarks showing the standard approach is insufficient, this complexity isn't justified. The Rust compiler with `-C target-cpu=native` often generates equivalent SIMD code for simple loops.
- **Missing safety documentation**: `unsafe` function without documenting invariants.
- **Platform-specific without fallback**: No implementation for non-x86_64.

## Action items
- [ ] Benchmark against `values.iter().sum()` to verify benefit
- [ ] If SIMD is truly needed, consider using `packed_simd` or `std::simd` (nightly)
- [ ] Add fallback for other architectures
- [ ] Document safety requirements

### Simpler alternative

```rust
// Let the compiler vectorize
pub fn sum(values: &[f64]) -> f64 {
    values.iter().sum()
}

// If you've proven this is a bottleneck and auto-vectorization fails:
// 1. Check compiler flags: -C target-cpu=native
// 2. Use portable SIMD crate instead of intrinsics
// 3. Document why manual SIMD is necessary with benchmark data
```

---

## Quick reference: Red flags

| Pattern | Why it's a problem | Quick fix |
|---------|-------------------|-----------|
| `.clone()` in iterator chain | Unnecessary allocation | Use references, restructure ownership |
| `format!()` in loop | Allocation per iteration | Pre-compute or reuse buffer |
| `Vec::new()` in loop | Allocation per iteration | Move outside loop, clear + reuse |
| `.collect()` then iterate | Intermediate allocation | Chain iterators directly |
| Nested loops with lookup | O(n²) or worse | Use HashMap/HashSet |
| `Box<T>` for small T | Heap allocation + indirection | Store inline |
| String keys in hot path | Allocation + hashing overhead | Use numeric IDs |
| Sequential loop over independent items | Missed parallelism | Use rayon par_iter |
| Manual SIMD without benchmarks | Complexity without proven benefit | Trust auto-vectorization first |
