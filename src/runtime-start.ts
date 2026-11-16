import type { NetworkMode, RuntimeStatus } from "./types";

export type RuntimeStartMode = "system_proxy" | "tun" | "previous";

/** Convert a visible toggle into an absolute intent. The backend rechecks
 *  the mode before a delayed off click may stop the current core. */
export function networkToggleIntent(mode: "system_proxy" | "tun", active: boolean):
    { action: "stop" | "switch"; mode: NetworkMode } {
  return { action: active ? "stop" : "switch", mode };
}

export function canStartRuntime(runtime: RuntimeStatus | null): boolean {
  return Boolean(runtime && ["uninitialized", "stopped", "crashed"].includes(runtime.phase));
}
