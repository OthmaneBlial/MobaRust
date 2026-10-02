export type RemoteFramebuffer = { width: number; height: number; pixels: Uint8ClampedArray<ArrayBuffer> };

export function decodeRemoteFramebuffer(data: ArrayBuffer): RemoteFramebuffer {
  const view = new DataView(data);
  if (data.byteLength < 14 || data.byteLength - 4 > 8 * 1024 * 1024
      || view.getUint32(0) !== data.byteLength - 4 || view.getUint32(4) !== 0x4d524642
      || view.getUint16(8) !== 2) throw new Error("The remote desktop sent an invalid framebuffer.");
  const width = view.getUint16(10);
  const height = view.getUint16(12);
  if (width < 320 || height < 200 || width > 16384 || height > 16384
      || width * height * 4 !== data.byteLength - 14) throw new Error("The remote desktop sent an invalid framebuffer.");
  return { width, height, pixels: new Uint8ClampedArray(data, 14) };
}

/** One fetch and one scheduled paint at a time; stale reconnect replies cannot paint. */
export class RemoteFramebufferQueue {
  private generation: number | null = null;
  private pending = false;
  private fetching = false;
  private animation: number | null = null;
  private epoch = 0;
  private disposed = false;
  private readonly fetch: (generation: number) => Promise<ArrayBuffer>;
  private readonly render: (frame: RemoteFramebuffer) => void;
  private readonly fail: (error: unknown) => void;

  constructor(
    fetch: (generation: number) => Promise<ArrayBuffer>,
    render: (frame: RemoteFramebuffer) => void,
    fail: (error: unknown) => void,
  ) { this.fetch = fetch; this.render = render; this.fail = fail; }

  request(generation: number) {
    if (this.disposed) return;
    if (!Number.isSafeInteger(generation) || generation < 0) {
      this.fail(new Error("The remote desktop sent an invalid framebuffer generation."));
      return;
    }
    this.generation = generation;
    this.refresh();
  }

  refresh() {
    if (this.disposed || this.generation === null) return;
    this.pending = true;
    if (this.fetching || this.animation !== null) return;
    this.animation = requestAnimationFrame(() => { this.animation = null; void this.flush(); });
  }

  reset() {
    this.epoch += 1;
    this.generation = null;
    this.pending = false;
    if (this.animation !== null) cancelAnimationFrame(this.animation);
    this.animation = null;
  }

  dispose() { this.disposed = true; this.reset(); }

  private async flush() {
    if (this.disposed || !this.pending || this.generation === null) return;
    const epoch = this.epoch;
    const generation = this.generation;
    this.pending = false;
    this.fetching = true;
    try {
      const data = await this.fetch(generation);
      if (!this.disposed && epoch === this.epoch && generation === this.generation && data.byteLength) this.render(decodeRemoteFramebuffer(data));
    } catch (error) {
      if (!this.disposed && epoch === this.epoch && generation === this.generation) this.fail(error);
    } finally {
      this.fetching = false;
      if (this.pending) this.refresh();
    }
  }
}
