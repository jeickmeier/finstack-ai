//! Stable high-volume pure-calculator benchmark.

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_tools_calculator::{Operation, evaluate};

fn calculator_throughput(criterion: &mut Criterion) {
    let operands = (1_u32..=256).map(f64::from).collect::<Vec<_>>();
    criterion.bench_function("calculator_add_256_operands", |bencher| {
        bencher.iter(|| evaluate(Operation::Add, &operands));
    });
}

criterion_group!(benches, calculator_throughput);
criterion_main!(benches);
