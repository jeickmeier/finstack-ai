# PR-063 optimizations

Measured numbers did not justify reducer, RawJson, registry, provider, or
store-replay changes.

- `decide_accept_run` ~163 ns vs 5 µs / 25 µs.
- `instant_model_request_poll` is the boxed ADR-030 dispatch; no unbox.
- Reducer ~165 µs vs PR-020 `--quick` ~161 µs. Gate settings differ.
- Incremental idle RSS ~65 KiB at 128 sessions is not framework-owned.
  `size_of::<RunTaskOwner>()` is 96 bytes. No determinism/safety ADR.

`PR-063-T-optimize-49f553fa621f` is Done as none required.
