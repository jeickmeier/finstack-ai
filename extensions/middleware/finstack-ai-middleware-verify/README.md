# finstack-ai-middleware-verify

Example `before_finalize` middleware. It can accept a candidate
(`Continue`), `Fail`, or `RequestInteraction`. It never writes a store
and cannot `Replace`, `AddInstructions`, `AddContext`, or
`CompactContext`.

> **`RequestInteraction` does not work.** That mode currently fails the
> run with `middleware_stage_unlandable` instead of prompting: an
> interaction does not consume a stage cursor, so the aggregate fold has
> no settlement that can carry it. `Continue` and `Fail` work. See
> [Shipping leaves](../../../docs/site/middleware.md#shipping-leaves).

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_middleware_verify::VerifyMiddleware;

let middleware = VerifyMiddleware::try_accept().expect("verify");
```
