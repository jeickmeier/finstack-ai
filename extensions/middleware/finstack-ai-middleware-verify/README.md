# finstack-ai-middleware-verify

Example `before_finalize` middleware. It can accept a candidate
(`Continue`), `Fail`, or `RequestInteraction`. It never writes a store
and cannot `Replace`, `AddInstructions`, `AddContext`, or
`CompactContext`.
