// Plain-Node tests for the framing layer (no VS Code needed): `npm test`.
import * as assert from "assert";
import { MessageReader, encode } from "../protocol";

const got: any[] = [];
const r = new MessageReader((m) => got.push(m));
const a = encode({ jsonrpc: "2.0", id: 1, result: { ok: true } });
const b = encode({ jsonrpc: "2.0", id: 2, result: { text: "héllo ✓" } });
const both = Buffer.concat([a, b]);
// Feed in awkward chunks, splitting inside headers and multibyte characters.
for (let i = 0; i < both.length; i += 7) r.push(both.subarray(i, i + 7));
assert.strictEqual(got.length, 2);
assert.strictEqual(got[0].id, 1);
assert.strictEqual(got[1].result.text, "héllo ✓");
console.log("protocol tests passed");
