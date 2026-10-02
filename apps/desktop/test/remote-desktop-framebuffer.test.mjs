import assert from "node:assert/strict";
import { setImmediate } from "node:timers";
import { log } from "node:console";
import { decodeRemoteFramebuffer, RemoteFramebufferQueue } from "../src/remote-desktop-framebuffer.ts";

function frame(width = 320, height = 200, red = 17) {
  const bytes = new ArrayBuffer(14 + width * height * 4);
  const view = new DataView(bytes);
  view.setUint32(0, bytes.byteLength - 4);
  view.setUint32(4, 0x4d524642);
  view.setUint16(8, 2);
  view.setUint16(10, width);
  view.setUint16(12, height);
  new Uint8Array(bytes, 14).set([red, 34, 51, 255]);
  return bytes;
}
const fullHd = frame(1920, 1080);
assert.equal(decodeRemoteFramebuffer(fullHd).pixels.buffer, fullHd);
assert.deepEqual([...decodeRemoteFramebuffer(fullHd).pixels.subarray(0, 4)], [17, 34, 51, 255]);
for (const offset of [0, 4, 8, 10, 12]) {
  const invalid = fullHd.slice(0);
  new DataView(invalid).setUint16(offset, 0);
  assert.throws(() => decodeRemoteFramebuffer(invalid));
}
assert.throws(() => decodeRemoteFramebuffer(fullHd.slice(0, -1)));
assert.throws(() => decodeRemoteFramebuffer(new ArrayBuffer(3)));
assert.throws(() => decodeRemoteFramebuffer(new ArrayBuffer(8 * 1024 * 1024 + 5)));

const animations = new Map();
let nextAnimation = 0;
globalThis.requestAnimationFrame = (callback) => { const id = ++nextAnimation; animations.set(id, callback); return id; };
globalThis.cancelAnimationFrame = (id) => animations.delete(id);
const tick = async () => { const callbacks = [...animations.values()]; animations.clear(); callbacks.forEach((callback) => callback(0)); await new Promise(setImmediate); };
const requests = [], paints = [], errors = [];
const queue = new RemoteFramebufferQueue(
  (generation) => new Promise((resolve, reject) => requests.push({ generation, resolve, reject })),
  (image) => paints.push(image.pixels[0]),
  (error) => errors.push(error),
);
for (let index = 0; index < 10000; index++) queue.request(0);
assert.equal(animations.size, 1);
await tick();
assert.equal(requests.length, 1);
for (let index = 0; index < 10000; index++) queue.request(0);
assert.equal(animations.size, 0);
assert.equal(requests.length, 1);
queue.reset();
queue.request(1);
requests[0].resolve(frame(320, 200, 17));
await new Promise(setImmediate);
assert.deepEqual(paints, []);
await tick();
assert.equal(requests[1].generation, 1);
requests[1].resolve(frame(320, 200, 68));
await new Promise(setImmediate);
assert.deepEqual(paints, [68]);
queue.request(1);
await tick();
queue.reset();
requests[2].reject(new Error("obsolete connection"));
await new Promise(setImmediate);
assert.deepEqual(errors, []);
queue.request(2);
await tick();
requests[3].resolve(new ArrayBuffer(0));
await new Promise(setImmediate);
queue.refresh();
await tick();
assert.equal(requests[4].generation, 2);
queue.dispose();
requests[4].resolve(frame());
await new Promise(setImmediate);
assert.deepEqual(paints, [68]);
assert.equal(animations.size, 0);
const invalidErrors = [];
const invalidQueue = new RemoteFramebufferQueue(
  async () => new ArrayBuffer(3),
  () => assert.fail("invalid frames must not paint"),
  (error) => invalidErrors.push(error),
);
for (const generation of [-1, NaN, 0.5, Number.MAX_SAFE_INTEGER + 1]) invalidQueue.request(generation);
assert.equal(invalidErrors.length, 4);
assert.equal(animations.size, 0);
invalidQueue.request(0);
await tick();
assert.equal(invalidErrors.length, 5);
invalidQueue.dispose();
log("Remote framebuffer bounds, coalescing and reconnect checks passed");
