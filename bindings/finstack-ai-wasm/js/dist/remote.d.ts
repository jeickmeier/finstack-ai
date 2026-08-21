/**
 * Length-prefix helpers for the remote/process frame (ADR-021).
 *
 * Body bytes stay canonical-CBOR produced by the Rust/WASM codec.
 * This module does not add a second JavaScript CBOR library.
 */
/** Pre-authentication frame ceiling in bytes (contract section 28.2). */
export declare const PRE_AUTH_FRAME_MAX_BYTES: number;
/** Default post-authentication frame ceiling in bytes. */
export declare const POST_AUTH_FRAME_MAX_BYTES: number;
/**
 * Encode `payload` as a 4-byte big-endian length plus the payload.
 *
 * @param payload - Canonical-CBOR envelope bytes from Rust.
 * @param ceiling - Maximum payload size checked before the frame is built.
 * @returns The framed bytes.
 * @throws {RangeError} When `payload` exceeds `ceiling`.
 * @example
 * ```ts
 * const frame = encodeFrame(new Uint8Array([1, 2]), PRE_AUTH_FRAME_MAX_BYTES);
 * ```
 */
export declare function encodeFrame(payload: Uint8Array, ceiling: number): Uint8Array;
/**
 * Read the declared payload length from a 4-byte header without allocating
 * the payload.
 *
 * @param header - Exactly four bytes.
 * @param ceiling - Maximum accepted payload size.
 * @returns The declared length.
 * @throws {RangeError} When the header is short or the length exceeds `ceiling`.
 */
export declare function decodeFrameLength(header: Uint8Array, ceiling: number): number;
/**
 * Split a complete frame into its payload.
 *
 * @param bytes - Length prefix plus payload.
 * @param ceiling - Maximum accepted payload size.
 * @returns The payload bytes.
 */
export declare function decodeFrame(bytes: Uint8Array, ceiling: number): Uint8Array;
//# sourceMappingURL=remote.d.ts.map