import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const [wasmPath, expectedPath] = process.argv.slice(2);
if (!wasmPath || !expectedPath) throw new Error("Expected Wasm module and native fingerprint JSON paths");
const expected: unknown = JSON.parse(await readFile(expectedPath, "utf8"));
if (typeof expected !== "object" || expected === null || !("sha256" in expected)
    || typeof expected.sha256 !== "string" || !("backend" in expected) || expected.backend !== "software") {
  throw new Error("Expected a software encoder fingerprint");
}
const { instance } = await WebAssembly.instantiate(await readFile(wasmPath));
const digestWord = instance.exports["h264_digest_word"];
if (typeof digestWord !== "function") throw new Error("Missing h264_digest_word export");
const bytes = Buffer.alloc(32);
for (let i = 0; i < 8; i++) bytes.writeUInt32LE(Number(digestWord(i)) >>> 0, i * 4);
const digest = bytes.toString("hex");
assert.equal(digest, expected.sha256, "Native and Wasm software AVC bytes differ");
process.stdout.write(`Software H.264 native/Wasm SHA-256 matched: ${digest}\n`);
