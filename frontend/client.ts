import { invoke } from "@tauri-apps/api/core";
import type { Action, Reply, Response } from "./contract";

// One running read and one replaceable pending read per view. Rapid selection
// never builds an obsolete subprocess backlog. The Rust queue is also bounded.
export class LatestLane {
  private active = false;
  private pending?: () => void;
  run(task: () => Promise<void>) {
    const start = async () => {
      this.active = true;
      try {
        await task();
      } finally {
        this.active = false;
        const next = this.pending;
        this.pending = undefined;
        next?.();
      }
    };
    if (this.active)
      this.pending = () => {
        void start();
      };
    else void start();
  }
}
export class Client {
  epoch = 0;
  sequence = 0;
  private current = new Map<string, number>();
  readonly lanes = {
    snapshot: new LatestLane(),
    details: new LatestLane(),
    diff: new LatestLane(),
  };
  invalidate(channel: string) {
    this.current.set(channel, ++this.sequence);
  }
  async request(action: Action, channel: string): Promise<Reply | undefined> {
    const start = performance.now();
    const epoch = this.epoch,
      sequence = ++this.sequence;
    this.current.set(channel, sequence);
    let response: Response;
    try {
      response = await invoke<Response>("execute", {
        request: { epoch, sequence, action },
      });
    } catch (error) {
      if (epoch !== this.epoch || this.current.get(channel) !== sequence)
        return;
      throw error;
    }
    performance.measure(`gitcrab.${action.kind}.roundtrip`, {
      start,
      end: performance.now(),
      detail: { worker_ms: response.elapsed_ms },
    });
    if (epoch !== this.epoch || this.current.get(channel) !== sequence) return;
    if (response.epoch !== epoch || response.sequence !== sequence)
      throw new Error("Invalid response identity. Restart GitCrab.");
    if ("Err" in response.result) throw response.result.Err;
    return response.result.Ok;
  }
}
export const client = new Client();
export function errorMessage(error: unknown): string {
  return typeof error === "object" && error && "message" in error
    ? String(error.message)
    : String(error);
}
