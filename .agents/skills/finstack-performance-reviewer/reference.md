# Performance Reference

## Algorithmic complexity guide

### Common data structure operations

| Operation | Vec | HashMap | BTreeMap | LinkedList |
|-----------|-----|---------|----------|------------|
| Index access | O(1) | N/A | N/A | O(n) |
| Search | O(n) | O(1)* | O(log n) | O(n) |
| Insert (end) | O(1)* | O(1)* | O(log n) | O(1) |
| Insert (middle) | O(n) | N/A | O(log n) | O(1)** |
| Remove | O(n) | O(1)* | O(log n) | O(1)** |
| Iteration | O(n) | O(n) | O(n) | O(n) |

*Amortized. **If you have a reference to the node.

### When to use which

- **Vec**: Default choice. Cache-friendly, fast iteration. Use for most sequences.
- **HashMap**: Fast key lookup when order doesn't matter. Use `FxHashMap` for non-cryptographic keys.
- **BTreeMap**: When you need sorted keys or range queries. Higher constant factor than HashMap.
- **LinkedList**: Almost never. Vec with remove-swap is usually better.

### Hidden complexity traps

```rust
// O(n²) - repeated Vec::contains
for item in &items {
    if !seen.contains(item) {  // O(n) per call
        seen.push(item.clone());
    }
}
// Fix: Use HashSet for O(n) total

// O(n²) - Vec::remove in loop
while let Some(idx) = find_bad_item(&items) {
    items.remove(idx);  // O(n) per removal
}
// Fix: Use retain() for O(n) total

// O(n²) - string concatenation
let mut result = String::new();
for s in strings {
    result = result + &s;  // Reallocates each time
}
// Fix: Use String::with_capacity + push_str, or collect with join
```

## Memory allocation patterns

### Stack vs heap decision tree

```
Is the size known at compile time?
├─ Yes: Can it fit on stack (< ~1MB)?
│  ├─ Yes → Use stack allocation (array, struct)
│  └─ No → Use Box or Vec
└─ No: Is it small and short-lived?
   ├─ Yes → Consider smallvec or arrayvec
   └─ No → Use Vec/Box with capacity hint
```

### Allocation-free patterns

```rust
// BAD: Allocates in loop
for i in 0..n {
    let temp = vec![0.0; size];  // New allocation each iteration
    process(&temp);
}

// GOOD: Reuse buffer
let mut temp = vec![0.0; size];
for i in 0..n {
    temp.fill(0.0);  // Reset without reallocating
    process(&temp);
}

// BAD: Unnecessary String allocation
fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

// GOOD: Accept pre-allocated buffer
fn greet_into(name: &str, buf: &mut String) {
    buf.clear();
    buf.push_str("Hello, ");
    buf.push_str(name);
    buf.push('!');
}
```

### Capacity hints

```rust
// BAD: Multiple reallocations
let mut v = Vec::new();
for i in 0..1000 {
    v.push(compute(i));  // Reallocates ~10 times
}

// GOOD: Single allocation
let mut v = Vec::with_capacity(1000);
for i in 0..1000 {
    v.push(compute(i));  // No reallocations
}

// BEST: Use collect with size hint
let v: Vec<_> = (0..1000).map(compute).collect();  // Rust infers capacity
```

## Cache efficiency

### Memory hierarchy latency

| Level | Latency | Size (typical) |
|-------|---------|----------------|
| L1 cache | ~1 ns | 32-64 KB |
| L2 cache | ~4 ns | 256 KB - 1 MB |
| L3 cache | ~12 ns | 2-32 MB |
| RAM | ~100 ns | GBs |

### Cache-friendly patterns

```rust
// BAD: Pointer chasing (cache hostile)
struct Node {
    value: f64,
    next: Option<Box<Node>>,
}

// GOOD: Contiguous storage (cache friendly)
struct Values {
    data: Vec<f64>,
}

// BAD: Array of Structs with unused fields
struct Particle {
    position: [f64; 3],
    velocity: [f64; 3],
    color: [f64; 4],      // Not used in physics
    texture_id: u32,      // Not used in physics
}
let particles: Vec<Particle> = ...;
// Physics iteration loads color/texture into cache unnecessarily

// GOOD: Struct of Arrays for hot/cold separation
struct Particles {
    positions: Vec<[f64; 3]>,   // Hot data together
    velocities: Vec<[f64; 3]>,  // Hot data together
    colors: Vec<[f64; 4]>,      // Cold data separate
    texture_ids: Vec<u32>,      // Cold data separate
}
```

### Struct layout optimization

```rust
// BAD: 24 bytes due to padding
struct Bad {
    a: u8,   // 1 byte + 7 padding
    b: u64,  // 8 bytes
    c: u8,   // 1 byte + 7 padding
}

// GOOD: 16 bytes, fields ordered by alignment
struct Good {
    b: u64,  // 8 bytes (8-byte aligned)
    a: u8,   // 1 byte
    c: u8,   // 1 byte + 6 padding
}

// Use #[repr(C)] when layout matters for FFI
// Use #[repr(packed)] sparingly (can cause unaligned access)
```

## Numerical computation

### Vectorization checklist

For SIMD auto-vectorization, ensure:

1. Simple loop structure (no early exits, no complex control flow)
2. Contiguous memory access (prefer `&[f64]` over iterators with complex chains)
3. No loop-carried dependencies (each iteration independent)
4. Aligned data (use `#[repr(align(32))]` for AVX)

```rust
// Vectorizable
fn dot_product(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// NOT vectorizable (loop-carried dependency)
fn running_sum(a: &[f64]) -> Vec<f64> {
    let mut sum = 0.0;
    a.iter().map(|x| { sum += x; sum }).collect()
}
```

### Precision vs performance

| Type | Precision | Performance notes |
|------|-----------|-------------------|
| f32 | ~7 digits | 2× throughput of f64 with SIMD |
| f64 | ~15 digits | Standard for financial calculations |

Use f32 only when:
- Precision requirements are clearly < 7 digits
- Memory bandwidth is the bottleneck
- SIMD throughput gain is measured and significant

### Transcendental function costs

Relative cost (approximate):
- `+`, `-`, `*`: 1×
- `/`: 5-10×
- `sqrt`: 5-15×
- `exp`, `ln`: 20-50×
- `sin`, `cos`: 30-100×
- `pow`: 50-100×

```rust
// BAD: Redundant exp calls
for i in 0..n {
    result[i] = a * (-rate * t[i]).exp() + b * (-rate * t[i]).exp();
}

// GOOD: Factor out common computation
for i in 0..n {
    let discount = (-rate * t[i]).exp();
    result[i] = (a + b) * discount;
}
```

## Concurrency patterns

### When to parallelize

Parallelize when:
- Work per iteration > ~1μs (parallel overhead)
- Iterations are independent (no shared mutable state)
- Data is large enough (> ~10K elements for simple ops)

```rust
// Sequential baseline
let results: Vec<_> = items.iter().map(expensive_compute).collect();

// Parallel with rayon (simple, usually correct choice)
use rayon::prelude::*;
let results: Vec<_> = items.par_iter().map(expensive_compute).collect();
```

### Lock contention patterns

```rust
// BAD: Coarse-grained lock
let data = Arc::new(Mutex::new(HashMap::new()));
// All threads contend on single lock

// BETTER: Fine-grained locking
let data = Arc::new(DashMap::new());
// Or use sharded HashMap

// BEST: Lock-free where possible
let data = Arc::new(AtomicU64::new(0));
```

### False sharing

```rust
// BAD: Adjacent atomics on same cache line
struct Counters {
    counter_a: AtomicU64,  // Thread A writes
    counter_b: AtomicU64,  // Thread B writes (same cache line!)
}

// GOOD: Pad to separate cache lines
#[repr(align(64))]  // Cache line size
struct PaddedCounter(AtomicU64);

struct Counters {
    counter_a: PaddedCounter,
    counter_b: PaddedCounter,
}
```

## Benchmarking guidance

### What to benchmark

1. **Representative workloads**: Use realistic data sizes and distributions
2. **Hot paths**: Focus on code that profiles show is slow
3. **Baseline comparison**: Always compare against current implementation

### Criterion.rs patterns

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_algorithm(c: &mut Criterion) {
    let data = setup_test_data();  // Outside benchmark loop

    c.bench_function("algorithm_name", |b| {
        b.iter(|| {
            algorithm(black_box(&data))  // black_box prevents optimization
        })
    });
}

// Compare implementations
fn bench_comparison(c: &mut Criterion) {
    let data = setup_test_data();

    let mut group = c.benchmark_group("comparison");
    group.bench_function("impl_a", |b| b.iter(|| impl_a(black_box(&data))));
    group.bench_function("impl_b", |b| b.iter(|| impl_b(black_box(&data))));
    group.finish();
}
```

### Common benchmarking mistakes

- **Measuring cold cache**: Run warmup iterations first
- **Dead code elimination**: Use `black_box` on inputs and outputs
- **Unrealistic data**: Use production-representative sizes and patterns
- **Ignoring variance**: Look at distributions, not just means
- **Micro-benchmarks only**: Also measure end-to-end latency

## Tools

### Profiling

- **perf** (Linux): Low-overhead sampling profiler
- **Instruments** (macOS): Built-in profiling suite
- **cargo-flamegraph**: Easy flame graph generation
- **DHAT** (via valgrind): Heap profiling

### Static analysis

- **cargo clippy**: Includes performance lints
- **cargo-bloat**: Analyze binary size
- **cargo-geiger**: Audit unsafe usage

### Runtime analysis

- **criterion**: Statistical benchmarking
- **tracing**: Structured logging with timing
- **divan**: Modern benchmarking alternative
