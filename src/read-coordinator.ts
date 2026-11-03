export type ReadPriority = 'foreground' | 'background';
type Flight = { revision: number; promise: Promise<unknown>; priority: ReadPriority; started: boolean; waiters: number; start: () => void; cancel: () => void };

/** Coalesce reads only. Reserve capacity for interaction; never replay writes. */
export class ReadCoordinator {
  private revision = 0;
  private flights = new Map<string, Flight>();
  private queue: Flight[] = [];
  private active = 0;
  private backgroundActive = 0;
  constructor(private readonly timeoutMs = 15_000) {}
  invalidate() { this.revision++; }
  private pump() {
    while (this.active < 4) {
      let index = this.queue.findIndex(f => f.priority === 'foreground');
      if (index < 0 && this.backgroundActive < 2) index = 0;
      if (index < 0 || !this.queue.length) return;
      const flight = this.queue.splice(index, 1)[0];
      flight.start();
    }
  }
  private create(key: string, operation: () => Promise<unknown>, priority: ReadPriority): Flight {
    let resolve!: (value: unknown) => void, reject!: (error: unknown) => void;
    const promise = new Promise<unknown>((yes, no) => { resolve = yes; reject = no; });
    const flight: Flight = { revision: this.revision, promise, priority, started: false, waiters: 0,
      cancel: () => reject(new Error('读取已取消；未更改代理设置。')),
      start: () => {
        flight.started = true; this.active++;
        const background = flight.priority === 'background';
        if (background) this.backgroundActive++;
        // A timed-out *running* operation retains its slot until actual completion.
        void Promise.resolve().then(operation).then(resolve, reject).finally(() => {
          this.active--; if (background) this.backgroundActive--; this.pump();
        });
      },
    };
    this.flights.set(key, flight);
    const clear = () => { if (this.flights.get(key) === flight) this.flights.delete(key); };
    void promise.then(clear, clear);
    this.queue.push(flight);
    return flight;
  }
  async read<T>(key: string, operation: () => Promise<T>, priority: ReadPriority = 'foreground'): Promise<T> {
    const deadline = Date.now() + this.timeoutMs;
    for (;;) {
      let flight = this.flights.get(key);
      if (!flight) {
        if (this.queue.length >= 128) throw new Error('状态读取繁忙，请稍后刷新。');
        flight = this.create(key, operation, priority);
      }
      if (priority === 'foreground') flight.priority = 'foreground';
      flight.waiters++;
      this.pump();
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
      } finally {
        clearTimeout(timer);
        flight.waiters--;
        if (!flight.started && flight.waiters === 0) {
          this.queue = this.queue.filter(queued => queued !== flight);
          if (this.flights.get(key) === flight) this.flights.delete(key);
          flight.cancel();
        }
      }
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
