/** Async work belongs to one visible interaction, never to a future reopen. */
export class InteractionSession {
  private generation = 0;
  private context: string | null = null;
  private pending = false;
  begin(context: string): number | null {
    if (this.context === context) return null;
    this.context = context;
    this.pending = true;
    return ++this.generation;
  }
  accepts(ticket: number, context: string): boolean {
    return ticket === this.generation && this.context === context;
  }
  refresh(context: string): number | null {
    if (this.context !== context || this.pending) return null;
    this.pending = true;
    return ++this.generation;
  }
  settle(ticket: number): void { if (ticket === this.generation) this.pending = false; }
  close(): void { this.generation++; this.context = null; this.pending = false; }
  isCurrent(context: string): boolean { return this.context === null || this.context === context; }
}
