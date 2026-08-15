# runtime-events fixtures

Durable-derived runtime-event compatibility fixtures for the 1.0 freeze.

```text
v1/<kind>/(valid|invalid)--<slug>.json
```

Unknown state-bearing fields fail closed. Durable event order is part of
the contract; `order/invalid--durable-order-swap.json` documents a
deliberate swap that must not be accepted as the canonical sequence.
Transient progress events stay non-replay-stable.
