import type { NetworkContextSnapshot } from './types';

/** A UI read never changes routes. Revisions are process-local and monotonic;
 * late IPC replies cannot revive cached measurements from a previous network. */
export function createNetworkContext(options: {
  read(): Promise<NetworkContextSnapshot>;
  changed(state: NetworkContextSnapshot | null, invalidated: boolean): void;
  now?(): number;
}) {
  let state: NetworkContextSnapshot | null = null, received = 0;
  let flight: Promise<void> | null = null;
  const now = options.now ?? Date.now;
  function current() { return state && now() - received <= 20_000 && now() >= received && !state.stale && state.observed ? state : null; }
  function accept(next: NetworkContextSnapshot) {
    if (state && next.generation < state.generation) return;
    const invalidated = state !== null && (state.revision !== next.revision || !current() || next.stale);
    state = next; received = now(); options.changed(state, invalidated);
  }
  function refresh() {
    if (flight) return flight;
    if (state && !current()) options.changed(null, true);
    flight = options.read().then(accept).catch(() => {
      if (!current()) options.changed(null, true);
    }).finally(() => { flight = null; });
    return flight;
  }
  return { current, accept, refresh };
}
