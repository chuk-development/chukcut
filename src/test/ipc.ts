/**
 * A fake Rust core.
 *
 * Every module's `lib/` folder reaches the backend through `invoke()`, so a
 * test that cannot control `invoke()` cannot test anything past the first
 * await. This installs Tauri's sanctioned mock transport
 * (`@tauri-apps/api/mocks`) and wraps it in three things a test actually needs:
 *
 * - **Scripted answers.** `handle` returns a value, `fail` rejects with a bare
 *   string, which is what `Result<T, String>` looks like on this side.
 * - **The payloads, as they crossed the boundary.** `calls()` returns the
 *   JSON-serialized arguments, so an assertion is against the wire shape —
 *   `segment_id` versus `segmentId` — and not against whatever object the
 *   wrapper happened to build.
 * - **Channels.** `channel()` hands back the `Channel` a command was given and
 *   drives it through Tauri's real callback plumbing, so the `{ index, message }`
 *   envelope and message ordering are exercised rather than bypassed.
 *
 * A command with no answer scripted rejects loudly and is recorded in
 * `unhandled`. Silence would be worse: several call sites deliberately swallow
 * IPC errors, and a forgotten stub would read as passing behaviour.
 */

import { Channel } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockConvertFileSrc, mockIPC, mockWindows } from "@tauri-apps/api/mocks";

// `@tauri-apps/api` declares `__TAURI_EVENT_PLUGIN_INTERNALS__` itself but not
// this one, because nothing in the public API is supposed to reach for it.
// Driving a Channel from the outside is exactly what a test has to do.
declare global {
  interface Window {
    __TAURI_INTERNALS__?: {
      invoke?: (command: string, args?: unknown, options?: unknown) => Promise<unknown>;
      transformCallback?: (callback: unknown, once?: boolean) => number;
      runCallback?: (id: number, data: unknown) => void;
      unregisterCallback?: (id: number) => void;
      metadata?: unknown;
    };
  }
}

export type IpcPayload = Record<string, unknown>;

export interface IpcCall {
  command: string;
  /** The arguments as JSON, exactly as they cross IPC. A `Channel` becomes `"__CHANNEL__:<id>"`. */
  payload: IpcPayload;
  /** The same arguments before serialization, so the live `Channel` can be recovered. */
  args: IpcPayload;
}

/** A scripted answer: a value, or a function of the payload and how many times it has been asked. */
export type Responder<T> = T | ((payload: IpcPayload, callIndex: number) => T | Promise<T>);

/** The Rust end of a `Channel<T>` handed to a command. */
export interface TestChannel {
  readonly id: number;
  /** Send one message, in order, the way `Channel::send` does. */
  emit(message: unknown): void;
  /** Send at an explicit index, to exercise out-of-order delivery. */
  emitAt(index: number, message: unknown): void;
  /** Drop the Rust end. */
  close(): void;
}

export interface IpcHarness {
  /** Answer `command` with `responder`. Replaces any previous answer. */
  handle<T>(command: string, responder: Responder<T>): IpcHarness;
  /** Reject `command` the way Rust does: with a bare string, not an `Error`. */
  fail(command: string, message: string | ((payload: IpcPayload) => string)): IpcHarness;
  /** Every payload sent for `command`, in order, as it crossed the boundary. */
  calls(command: string): IpcPayload[];
  /** The most recent payload for `command`, or `undefined` if it was never called. */
  lastCall(command: string): IpcPayload | undefined;
  /** How many times `command` was invoked. */
  count(command: string): number;
  /** Every call, across all commands, in order. */
  readonly log: readonly IpcCall[];
  /** Commands that were invoked with no answer scripted. Should be empty. */
  readonly unhandled: readonly string[];
  /**
   * The `Channel` given to a call of `command`. `index` counts from the end, so
   * the default `-1` is the most recent — which is the one a restart just opened.
   */
  channel(command: string, index?: number): TestChannel;
  /** Emit a window-level Tauri event, e.g. `tauri://drag-drop`. */
  emitWindowEvent(name: string, payload: unknown): Promise<void>;
  /** Forget recorded calls. Scripted answers stay. */
  reset(): void;
  /** Uninstall. Call from `afterEach`. */
  restore(): void;
}

export interface InstallIpcOptions {
  /**
   * The window/webview label to publish as metadata. `null` leaves the metadata
   * absent, which is what makes `getCurrentWebview()` throw — the state the
   * white-screen regression was found in.
   */
  webviewLabel?: string | null;
}

export function installIpc(options: InstallIpcOptions = {}): IpcHarness {
  const responders = new Map<string, Responder<unknown>>();
  const failures = new Map<string, string | ((payload: IpcPayload) => string)>();
  const log: IpcCall[] = [];
  const unhandled: string[] = [];
  const channelIndex = new Map<number, number>();

  mockIPC(
    (command, args) => {
      const raw = (args ?? {}) as IpcPayload;
      const call: IpcCall = { command, payload: serialize(raw), args: raw };
      log.push(call);

      const failure = failures.get(command);
      if (failure !== undefined) {
        return Promise.reject(typeof failure === "function" ? failure(call.payload) : failure);
      }

      if (!responders.has(command)) {
        unhandled.push(command);
        return Promise.reject(
          `no answer scripted for the IPC command "${command}" — add ipc.handle("${command}", …)`,
        );
      }

      const responder = responders.get(command);
      const callIndex = log.filter((entry) => entry.command === command).length - 1;
      return Promise.resolve(
        typeof responder === "function"
          ? (responder as (payload: IpcPayload, index: number) => unknown)(call.payload, callIndex)
          : responder,
      );
    },
    { shouldMockEvents: true },
  );

  const label = options.webviewLabel === undefined ? "main" : options.webviewLabel;
  if (label !== null) mockWindows(label);
  mockConvertFileSrc("linux");

  const harness: IpcHarness = {
    handle(command, responder) {
      failures.delete(command);
      responders.set(command, responder as Responder<unknown>);
      return harness;
    },
    fail(command, message) {
      responders.delete(command);
      failures.set(command, message);
      return harness;
    },
    calls(command) {
      return log.filter((entry) => entry.command === command).map((entry) => entry.payload);
    },
    lastCall(command) {
      const matching = harness.calls(command);
      return matching[matching.length - 1];
    },
    count(command) {
      return harness.calls(command).length;
    },
    log,
    unhandled,
    channel(command, index = -1) {
      const matching = log.filter((entry) => entry.command === command);
      const call = index < 0 ? matching[matching.length + index] : matching[index];
      if (!call) {
        throw new Error(`"${command}" was called ${matching.length} times; no call at ${index}`);
      }
      const found = Object.values(call.args).find((value) => value instanceof Channel);
      if (!found) throw new Error(`the call to "${command}" carried no Channel`);
      return wrapChannel(found as Channel<unknown>, channelIndex);
    },
    async emitWindowEvent(name, payload) {
      await emit(name, payload);
    },
    reset() {
      log.length = 0;
      unhandled.length = 0;
      channelIndex.clear();
    },
    restore() {
      clearMocks();
      // React runs an effect cleanup after the test that mounted the component
      // has already finished — `unlisten()` is the usual one, and it invokes.
      // Inert stubs turn that late teardown into a no-op instead of an
      // unhandled rejection landing in the next test's output. The webview
      // metadata stays deleted, so `getCurrentWebview()` still throws.
      window.__TAURI_INTERNALS__ ??= {};
      const internals = window.__TAURI_INTERNALS__;
      internals.invoke = () => Promise.resolve(null);
      internals.transformCallback = () => 0;
      internals.runCallback = () => {};
      internals.unregisterCallback = () => {};
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    },
  };

  return harness;
}

function wrapChannel(channel: Channel<unknown>, indices: Map<number, number>): TestChannel {
  const deliver = (index: number, body: Record<string, unknown>) => {
    const run = window.__TAURI_INTERNALS__?.runCallback;
    if (!run) throw new Error("the IPC mock is not installed");
    run(channel.id, { index, ...body });
  };
  return {
    id: channel.id,
    emit(message) {
      const next = indices.get(channel.id) ?? 0;
      indices.set(channel.id, next + 1);
      deliver(next, { message });
    },
    emitAt(index, message) {
      indices.set(channel.id, Math.max(indices.get(channel.id) ?? 0, index + 1));
      deliver(index, { message });
    },
    close() {
      deliver(indices.get(channel.id) ?? 0, { end: true });
    },
  };
}

/**
 * What the arguments look like on the other side of the boundary.
 *
 * `Channel` collapses to its `__CHANNEL__:<id>` marker through `toJSON`, and
 * `undefined` disappears — both of which are exactly what Rust would see, and
 * both of which a `toEqual` against the original object would quietly miss.
 */
function serialize(payload: IpcPayload): IpcPayload {
  return JSON.parse(JSON.stringify(payload)) as IpcPayload;
}
