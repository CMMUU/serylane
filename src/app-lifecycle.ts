export interface LifecycleSnapshot {
  generation: number;
  phase: "open" | "preparing" | "updating" | "closing";
}

/** Native events can arrive out of order; cancellation starts a new epoch. */
export class AppLifecycle {
  private snapshot: LifecycleSnapshot = { generation: 0, phase: "open" };
  get paused(): boolean { return this.snapshot.phase !== "open"; }
  get generation(): number { return this.snapshot.generation; }
  update(next: LifecycleSnapshot): boolean {
    if (!Number.isSafeInteger(next.generation) || next.generation < this.snapshot.generation ||
        !["open", "preparing", "updating", "closing"].includes(next.phase) || this.snapshot.phase === "closing") return false;
    if (next.generation === this.snapshot.generation) return false;
    this.snapshot = { ...next };
    return true;
  }
  async read<T>(operation: () => Promise<T>): Promise<T> {
    this.requireOpen();
    const generation = this.generation;
    const result = await operation();
    this.requireOpen();
    if (generation !== this.generation) throw new Error("会话状态已变化，请重新刷新。");
    return result;
  }
  requireStatus(): void {
    // The in-memory updater snapshot must remain visible during installation.
    if (this.snapshot.phase !== "updating") this.requireOpen();
  }
  requireOpen(): void {
    if (this.snapshot.phase === "updating") throw new Error("正在安装更新，已暂停新操作；安装结束后可继续使用。");
    if (this.paused) throw new Error("系统正在结束会话，已暂停新操作；取消关机后可继续使用。");
  }
}

export const appLifecycle = new AppLifecycle();
