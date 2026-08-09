---
name: finstack-performance-reviewer
description: Reviews code for performance issues focusing on algorithmic complexity, memory allocation, cache efficiency, numerical computation, and concurrency. Prioritizes simple, high-impact optimizations over micro-optimizations. Use when reviewing performance-sensitive code, optimizing hot paths, or when the user mentions performance, speed, latency, throughput, or memory efficiency.
---

# Performance Reviewer

## Quick start

When reviewing code for performance, produce a review with:

1. **Summary**: what code is being reviewed, performance risk level.
2. **Performance concerns**: 3–7 bullets on the most impactful issues.
3. **Findings**: grouped by severity with concrete fixes and expected impact.
4. **Action items**: checklist prioritized by impact-to-effort ratio.

After each review cycle, re-check the code and update the review. Continue iterating until there are no remaining action items.

**Philosophy**: Optimize for simplicity first, performance second. The fastest code is often the simplest code. Avoid premature optimization—measure before optimizing, and only optimize what matters.

Use this skill, not a separate benchmark skill, for benchmark regression review. Open the benchmark resources when the task mentions Criterion, portfolio-scale runtime, release-profile Python builds, flamegraphs, allocation profiling, or baseline comparisons.

## Severity rubric

- **Blocker**: Algorithmic complexity that makes code unusable at scale (O(n²) where O(n) is obvious), unbounded memory growth, deadlocks, blocking in async contexts.
- **Major**: Unnecessary allocations in hot paths, cache-hostile patterns, suboptimal algorithm choice with measurable impact, missing parallelization opportunities.
- **Minor**: Small allocation inefficiencies, iterator chain improvements, missed SIMD opportunities, suboptimal but correct patterns.
- **Nit**: Micro-optimizations with negligible real-world impact, style preferences.

## Core principles

### 1. Measure first, optimize second

- Never optimize without evidence (benchmarks, profiling, production metrics).
- Identify actual hot paths before touching code.
- Quantify expected vs actual improvement after changes.

### 2. Algorithmic wins beat micro-optimizations

- O(n) → O(log n) is worth 100 micro-optimizations.
- Fix the algorithm before tweaking the implementation.
- Simple algorithms with good constants often beat complex "optimal" ones.

### 3. Simple code is often fast code

- Clear, straightforward code is easier to optimize later.
- Compiler optimizations work better on simple patterns.
- Over-engineered "fast" code often isn't.

### 4. Allocation is the enemy

- Heap allocation is expensive; stack allocation is (nearly) free.
- Reuse buffers; avoid allocating in loops.
- Prefer borrowing over cloning.

## Review checklist

### Algorithmic complexity

- Identify time complexity of all operations, especially in loops.
- Look for hidden O(n²): nested iterations, repeated lookups in Vec, string concatenation.
- Check data structure choices: HashMap vs BTreeMap vs Vec for lookups.
- Verify sort/search algorithms match data characteristics.

### Memory allocation

- Flag allocations inside hot loops (Vec::new, String::new, Box, format!).
- Look for unnecessary clones: `.clone()`, `.to_string()`, `.to_vec()`.
- Check for opportunities to reuse buffers (`with_capacity`, clear + reuse).
- Identify ownership patterns that force unnecessary copies.
- Prefer `&str` over `String`, `&[T]` over `Vec<T>` in function signatures.

### Cache locality & data layout

- Prefer arrays/vectors over linked structures (Vec > LinkedList).
- Check struct field ordering for padding minimization.
- Identify pointer-chasing patterns (nested Box, Rc, Arc indirection).
- Look for AoS vs SoA opportunities in numerical code.
- Verify hot data fits in cache; consider data splitting.

### Numerical computation

- Check for vectorization opportunities (simple loops, no dependencies).
- Identify precision vs performance trade-offs (f32 vs f64).
- Look for redundant computations that can be hoisted or cached.
- Verify numerical algorithms use cache-efficient access patterns.
- Check for unnecessary transcendental functions (exp, log, sin) in hot paths.

### Concurrency & parallelism

- Identify embarrassingly parallel operations missing `rayon` or similar.
- Check for lock contention: coarse-grained locks, lock ordering.
- Look for false sharing in parallel code (cache line contention).
- Verify async code doesn't block the executor.
- Check atomic ordering: prefer `Relaxed` when sufficient, avoid `SeqCst` unless needed.

### I/O & serialization

- Identify synchronous I/O in performance-critical paths.
- Check buffer sizes for file/network I/O.
- Look for serialization/deserialization in hot paths.
- Verify batch operations where applicable (bulk DB queries, batch writes).
- Check for unnecessary string parsing/formatting.

### Benchmark regression

- Check whether a benchmark baseline exists and whether the changed code is on the measured path.
- Prefer release-profile measurements for PyO3 and portfolio-scale workloads.
- Distinguish wall-clock noise from material regressions; flag >10% regressions unless the repo has a stricter threshold.
- Pair every optimization recommendation with a benchmark or profiling command.

## Rust-specific patterns

| Issue | Symptom | Simple fix |
|-------|---------|------------|
| Unnecessary clone | `.clone()` on borrowed data | Restructure ownership or use `Cow` |
| Collect then iterate | `.collect::<Vec<_>>()` followed by iteration | Remove collect, chain iterators |
| String in loop | `format!` or `+` in loop | Use `String::with_capacity` + `push_str` |
| Vec without capacity | Growing Vec in loop | `Vec::with_capacity(expected_size)` |
| Bounds checking | Index access in tight loop | Use iterators or `get_unchecked` (with safety) |
| HashMap default hasher | SipHash for non-cryptographic keys | Use `FxHashMap` or `AHashMap` |
| Arc where Rc suffices | Arc in single-threaded context | Use Rc, or better, avoid indirection |
| Box<dyn Trait> in hot path | Dynamic dispatch overhead | Consider enum dispatch or generics |

## When NOT to optimize

Explicitly flag these anti-patterns:

- **Premature optimization**: No benchmark showing this code is hot.
- **Complexity for marginal gain**: 2% faster but 3× more complex.
- **Unsafe without justification**: Performance gain doesn't justify safety risk.
- **Platform-specific tricks**: Non-portable optimizations without fallback.

## Output template

```markdown
## Summary
<1–3 bullets: what code, performance risk level>

## Performance concerns
- <concern with expected impact>
- <algorithmic issue>
- <allocation pattern>

## Findings

### Blockers
- <issue> (fix, expected impact)

### Majors
- <issue> (fix, expected impact)

### Minors / Nits
- <improvement> (optional)

## Benchmarking recommendations
- <what to measure>
- <how to measure it>

## Action items
- [ ] <high-impact fix>
- [ ] <benchmark to add>
```

## Additional resources

- For detailed patterns and Rust-specific guidance, see [reference.md](reference.md).
- For code examples of common issues, see [examples.md](examples.md).
- For benchmark regression workflow, see [benchmark-regression.md](benchmark-regression.md).
- For finstack-quant hot-path examples, see [examples/finstack-hot-paths.md](examples/finstack-hot-paths.md).
