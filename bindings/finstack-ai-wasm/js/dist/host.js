/**
 * Trusted JavaScript host ABI for `@finstack/ai`.
 *
 * Host objects remain the contract (ADR-019). The wasm crate wraps them as
 * local `!Send` port implementations. IDs are strings; dynamic JSON crosses
 * the boundary as strings or `Uint8Array`.
 */
export {};
//# sourceMappingURL=host.js.map