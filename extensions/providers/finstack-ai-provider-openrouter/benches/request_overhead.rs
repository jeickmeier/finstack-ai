//! Public provider-path and catalog parsing benchmarks.

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai_provider_openrouter::{
    OpenRouterConfig, OpenRouterModelConfig, OpenRouterProvider, model_configs_from_catalog_json,
};
use finstack_ai_runtime::{Model, ModelName};

fn provider_estimate(criterion: &mut Criterion) {
    let config = OpenRouterConfig::try_new("http://127.0.0.1:9").expect("config");
    let model = OpenRouterModelConfig::try_new(
        "openai/gpt-bench",
        1_000_000,
        400_000,
        128_000,
        32_000,
        256,
    )
    .expect("model");
    let provider = OpenRouterProvider::try_new(config, vec![model]).expect("provider");
    let model_name = ModelName::try_new("openai/gpt-bench").expect("model name");
    let canonical_request = br#"{"messages":[{"role":"system","content":"Be precise and concise."},{"role":"user","content":"What moved oil prices this week?"}],"settings":{"temperature":0.7,"top_p":0.9}}"#;

    criterion.bench_function("openrouter/provider-estimate", |bencher| {
        bencher.iter(|| {
            std::hint::black_box(
                provider
                    .estimate_input_tokens(
                        std::hint::black_box(&model_name),
                        std::hint::black_box(canonical_request),
                    )
                    .expect("estimate"),
            );
        });
    });
}

fn catalog_parse(criterion: &mut Criterion) {
    let body = bench_catalog_json();
    criterion.bench_function("openrouter/catalog-parse", |bencher| {
        bencher.iter(|| {
            std::hint::black_box(
                model_configs_from_catalog_json(std::hint::black_box(body.as_bytes()), 1_000_000)
                    .expect("catalog"),
            );
        });
    });
}

fn bench_catalog_json() -> String {
    let entries = (0..200)
        .map(|index| {
            let context_length = 32_768 + (index % 8) * 32_768;
            let max_completion_tokens = 4_096 + (index % 4) * 4_096;
            format!(
                r#"{{"id":"vendor-{index}/model-{index}","context_length":{context_length},"top_provider":{{"max_completion_tokens":{max_completion_tokens}}},"supported_parameters":["tools"],"architecture":{{"input_modalities":["text"]}}}}"#
            )
        })
        .collect::<Vec<_>>();
    format!(r#"{{"data":[{}]}}"#, entries.join(","))
}

criterion_group!(benches, provider_estimate, catalog_parse);
criterion_main!(benches);
