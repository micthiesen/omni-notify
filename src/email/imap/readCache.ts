/** Small insertion-ordered cache with explicit entry and byte bounds. */
export class BoundedReadCache<V> {
  private readonly entries = new Map<
    string,
    { value: V; bytes: number; expiresAt: number }
  >();
  private bytes = 0;

  constructor(
    private readonly maxEntries: number,
    private readonly maxBytes: number,
    private readonly ttlMs = Number.POSITIVE_INFINITY,
  ) {}

  get(key: string, now: number): V | undefined {
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    if (entry.expiresAt <= now) {
      this.remove(key);
      return undefined;
    }
    this.entries.delete(key);
    this.entries.set(key, entry);
    return entry.value;
  }

  set(key: string, value: V, bytes: number, now: number): void {
    this.remove(key);
    if (bytes > this.maxBytes || this.maxEntries < 1) return;
    while (this.entries.size >= this.maxEntries || this.bytes + bytes > this.maxBytes) {
      const oldest = this.entries.keys().next().value as string | undefined;
      if (oldest === undefined) break;
      this.remove(oldest);
    }
    this.entries.set(key, { value, bytes, expiresAt: now + this.ttlMs });
    this.bytes += bytes;
  }

  clear(): void {
    this.entries.clear();
    this.bytes = 0;
  }

  delete(key: string): void {
    this.remove(key);
  }

  private remove(key: string): void {
    const entry = this.entries.get(key);
    if (!entry) return;
    this.entries.delete(key);
    this.bytes -= entry.bytes;
  }
}
