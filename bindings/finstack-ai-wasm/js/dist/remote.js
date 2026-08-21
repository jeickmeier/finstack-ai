/**
 * Length-prefix helpers for the remote/process frame (ADR-021).
 *
 * Body bytes stay canonical-CBOR produced by the Rust/WASM codec.
 * This module does not add a second JavaScript CBOR library.
 */
/** Pre-authentication frame ceiling in bytes (contract section 28.2). */
export const PRE_AUTH_FRAME_MAX_BYTES = 16 * 1024;
/** Default post-authentication frame ceiling in bytes. */
export const POST_AUTH_FRAME_MAX_BYTES = 256 * 1024;
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
export function encodeFrame(payload, ceiling) {
    if (payload.byteLength > ceiling) {
        throw new RangeError(`frame_payload exceeds ${ceiling}`);
    }
    const frame = new Uint8Array(4 + payload.byteLength);
    const view = new DataView(frame.buffer);
    view.setUint32(0, payload.byteLength, false);
    frame.set(payload, 4);
    return frame;
}
/**
 * Read the declared payload length from a 4-byte header without allocating
 * the payload.
 *
 * @param header - Exactly four bytes.
 * @param ceiling - Maximum accepted payload size.
 * @returns The declared length.
 * @throws {RangeError} When the header is short or the length exceeds `ceiling`.
 */
export function decodeFrameLength(header, ceiling) {
    if (header.byteLength < 4) {
        throw new RangeError("truncated_frame");
    }
    const view = new DataView(header.buffer, header.byteOffset, 4);
    const declared = view.getUint32(0, false);
    if (declared > ceiling) {
        throw new RangeError(`frame_payload exceeds ${ceiling}`);
    }
    return declared;
}
/**
 * Split a complete frame into its payload.
 *
 * @param bytes - Length prefix plus payload.
 * @param ceiling - Maximum accepted payload size.
 * @returns The payload bytes.
 */
export function decodeFrame(bytes, ceiling) {
    const declared = decodeFrameLength(bytes, ceiling);
    if (bytes.byteLength !== 4 + declared) {
        throw new RangeError("frame_length_mismatch");
    }
    return bytes.subarray(4);
}
//# sourceMappingURL=remote.js.map