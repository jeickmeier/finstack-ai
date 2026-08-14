# `@finstack/ai`

Preview browser package for the Rust-owned `finstack-ai` engine. The published
TypeScript surface is hand-authored; generated wasm-bindgen glue stays in
`generated/` and is not a public API.

## Install

This package is not published. Consume it from the repository checkout after
`mise run generate-wasm`.

```ts
import { buildMetadata, health, init } from "@finstack/ai";

await init();
health(); // "ok"
buildMetadata();
```

`init` loads the generated module. `health` and `buildMetadata` do not create a
runtime, open a store, or spawn work.

## Public surface (PR-033)

- `init(): Promise<void>`
- `health(): string`
- `buildMetadata(): { version, engineVersion, implementation: "wasm", target: "wasm32-unknown-unknown" }`

Agent, Run, Result, event-batch handles, JS model/tool/store adapters, workers,
and IndexedDB are later pull requests. Do not embed provider credentials in
browser bundles or examples.

## Regenerate

```bash
mise run generate-wasm
```

That command is the only supported regeneration path. It writes `generated/`
and `dist/`. CI regenerates and fails on a dirty tree.

## License

`MIT OR Apache-2.0`. Canonical texts live under [`../../../licenses/`](../../../licenses/)
and are copied next to this README.
