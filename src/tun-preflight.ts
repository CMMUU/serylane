import type { TunHelperStatus } from "./types";

export type TunPreflightResult =
  | { kind: "ready" }
  | { kind: "blocked"; status: TunHelperStatus }
  | { kind: "failed"; error: unknown };

type Context = {
  platform: string;
  status(): Promise<TunHelperStatus>;
  install(): Promise<TunHelperStatus>;
  openApproval(): Promise<unknown>;
  prepare(): Promise<unknown>;
  observed(status: TunHelperStatus): void;
};

/** Success is the fulfilled command, not its payload: native Rust () is null. */
export async function prepareTunForStart(context: Context): Promise<TunPreflightResult> {
  try {
    let status = await context.status();
    context.observed(status);
    if (!status.supported || !["macos", "windows"].includes(context.platform)) {
      return { kind: "blocked", status };
    }
    if (context.platform === "macos" && status.state === "not_installed") {
      status = await context.install();
      context.observed(status);
    }
    // A transient status read must never unregister/reinstall a privileged
    // service. Repair is a separate, explicit settings action on macOS only.
    if (!status.supported || status.state !== "ready") {
      if (context.platform === "macos" && status.state === "requires_approval") {
        await context.openApproval();
      }
      return { kind: "blocked", status };
    }
    await context.prepare();
    return { kind: "ready" };
  } catch (error) {
    return { kind: "failed", error };
  }
}
