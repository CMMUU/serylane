/** Coalesce reads only. Writes keep their existing transactional ordering. */
export class ReadCoordinator {
  private revision = 0;
  private flights = new Map<string, { revision: number; promise: Promise<unknown> }>();
  constructor(private readonly timeoutMs = 15_000) {}
  invalidate() { this.revision++; }
  async read<T>(key: string, operation: () => Promise<T>): Promise<T> {
    const deadline = Date.now() + this.timeoutMs;
    for (;;) {
      let flight = this.flights.get(key);
      if (!flight) {
        flight = { revision: this.revision, promise: Promise.resolve().then(operation) };
        this.flights.set(key, flight);
        const current = flight;
        const clear = () => { if (this.flights.get(key) === current) this.flights.delete(key); };
        void flight.promise.then(clear, clear);
      }
      let timer: ReturnType<typeof setTimeout> | undefined;
      try {
        const remaining = deadline - Date.now();
        if (remaining <= 0) throw new Error('状态读取超时，已保留当前显示，请稍后刷新。');
        const value = await Promise.race([
          flight.promise,
          new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('状态读取超时，已保留当前显示，请稍后刷新。')), remaining); }),
        ]);
        if (flight.revision === this.revision) return structuredClone(value) as T;
      } catch (error) {
        if (flight.revision === this.revision || Date.now() >= deadline) throw error;
      } finally { clearTimeout(timer); }
      // A mutation invalidated this response. Await completion before issuing a
      // fresh read, so even a timed-out native request cannot grow an IPC queue.
    }
  }
}

/** Coalesce the whole render operation, not only the underlying IPC. */
export class PageReads {
  private flights = new Map<string, { context: string; promise: Promise<unknown> }>();
  run<T>(key: string, context: string, operation: () => Promise<T>): Promise<T> {
    const current = this.flights.get(key);
    if (current) {
      if (current.context === context) return current.promise as Promise<T>;
      return current.promise.catch(() => {}).then(() => this.run(key, context, operation));
    }
    const promise = Promise.resolve().then(operation);
    const flight = { context, promise };
    this.flights.set(key, flight);
    const clear = () => { if (this.flights.get(key) === flight) this.flights.delete(key); };
    void promise.then(clear, clear);
    return promise;
  }
}
