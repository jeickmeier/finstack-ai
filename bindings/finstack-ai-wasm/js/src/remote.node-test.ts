import assert from "node:assert/strict";
import { test } from "node:test";

import {
  PRE_AUTH_FRAME_MAX_BYTES,
  decodeFrame,
  decodeFrameLength,
  encodeFrame,
} from "./remote.ts";

test("one-over pre-auth ceiling fails before payload allocation", () => {
  const header = new Uint8Array(4);
  new DataView(header.buffer).setUint32(0, PRE_AUTH_FRAME_MAX_BYTES + 1, false);
  assert.throws(() => decodeFrameLength(header, PRE_AUTH_FRAME_MAX_BYTES), RangeError);
});

test("round-trips a remote-sized payload without a JS CBOR codec", () => {
  const payload = new Uint8Array([0xa1, 0x64, 0x6b, 0x69, 0x6e, 0x64, 0x01]);
  const frame = encodeFrame(payload, PRE_AUTH_FRAME_MAX_BYTES);
  assert.equal(decodeFrameLength(frame, PRE_AUTH_FRAME_MAX_BYTES), payload.byteLength);
  assert.deepEqual(decodeFrame(frame, PRE_AUTH_FRAME_MAX_BYTES), payload);
});
