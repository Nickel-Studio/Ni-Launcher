import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { LaunchDiagnostic } from "../../../src/components/launch/useLauncher";

export interface Response {
  id?: string;
  hold?: boolean;
  value?: unknown;
  error?: string;
  delayMs?: number;
}

export interface Setup {
  mode?: "home" | "hook" | "dialog";
  accountType?: "offline" | "microsoft";
  displayName?: string;
  error?: string;
  diagnostic?: LaunchDiagnostic;
  listenDelayMs?: number;
  listenFailures?: number;
  responses?: Record<string, Response[]>;
}

interface Call {
  command: string;
  payload: Record<string, unknown>;
  at: number;
  outcome?: "resolved" | "rejected";
}

const wait = (ms: number) =>
  new Promise<void>((resolve) => window.setTimeout(resolve, ms));

export function installBackend(setup: Setup) {
  const settings = {
    accountType: setup.accountType || "offline",
    offlineUsername: "AcceptancePlayer",
    themeMode: "dark",
    accent: "nickel",
    titlebarTint: true,
  };
  localStorage.setItem("nickel.settings", JSON.stringify(settings));
  const account = {
    id: "fixture-player",
    type: "microsoft",
    name: "AcceptancePlayer",
  };
  localStorage.setItem(
    "nickel.accounts",
    JSON.stringify(setup.accountType === "microsoft" ? [account] : [])
  );
  localStorage.setItem(
    "nickel.accounts.active",
    setup.accountType === "microsoft" ? account.id : ""
  );
  mockWindows("main");
  const responses = Object.fromEntries(
    Object.entries(setup.responses || {}).map(([key, value]) => [
      key,
      [...value],
    ])
  );
  const pending = new Map<string, () => void>();
  const calls: Call[] = [];
  const listeners = new Map<string, Set<number>>();
  let listenCalls = 0;
  let listenFailures = setup.listenFailures || 0;

  mockIPC(
    async (command, raw) => {
      const payload =
        raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
      const entry: Call = { command, payload, at: performance.now() };
      calls.push(entry);
      const response = responses[command]?.shift();
      try {
        if (response?.hold) {
          const id = response.id || `${command}-${calls.length}`;
          await new Promise<void>((resolve) => pending.set(id, resolve));
        }
        if (response?.delayMs) await wait(response.delayMs);
        if (response?.error) throw new Error(response.error);
        if (response && "value" in response) {
          entry.outcome = "resolved";
          return response.value;
        }
        let value: unknown = null;
        switch (command) {
          case "settings_get":
            value = JSON.stringify(settings);
            break;
          case "settings_set":
            break;
          case "accounts_list":
            value = setup.accountType === "microsoft" ? [account] : [];
            break;
          case "accounts_set_active":
            break;
          case "app_tasks_snapshot":
            value = [];
            break;
          case "msa_config_get":
            value = {
              clientId: "",
              redirectUri: "http://localhost",
              configured: false,
            };
            break;
          case "skin_fetch":
            break;
          case "launch_game":
            break;
          case "launch_cancel":
            break;
          case "launch_log_open":
            break;
          case "launch_error_report_export":
            break;
          default:
            if (!command.startsWith("plugin:"))
              throw new Error(`Unconfigured fixture command: ${command}`);
        }
        entry.outcome = "resolved";
        return value;
      } catch (error) {
        entry.outcome = "rejected";
        throw error;
      }
    },
    { shouldMockEvents: true }
  );

  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (
          command: string,
          args?: Record<string, unknown>,
          options?: unknown
        ) => Promise<unknown>;
      };
    }
  ).__TAURI_INTERNALS__;
  const originalInvoke = internals.invoke;
  internals.invoke = async (command, args = {}, options) => {
    if (command === "plugin:event|listen") {
      listenCalls += 1;
      if (setup.listenDelayMs) await wait(setup.listenDelayMs);
      if (listenFailures > 0) {
        listenFailures -= 1;
        throw new Error("Simulated event-listener failure");
      }
      const id = (await originalInvoke(command, args, options)) as number;
      const name = String(args.event);
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name)!.add(id);
      return id;
    }
    if (command === "plugin:event|unlisten") {
      listeners.get(String(args.event))?.delete(args.eventId as number);
      return originalInvoke(command, { ...args, id: args.eventId }, options);
    }
    return originalInvoke(command, args, options);
  };

  return {
    emit: (payload: Record<string, unknown>) => emit("launcher-event", payload),
    release(id: string) {
      const resolve = pending.get(id);
      if (!resolve) throw new Error(`No pending fixture request: ${id}`);
      pending.delete(id);
      resolve();
    },
    snapshot: () => ({
      scope:
        "production-components-and-hook; simulated IPC; no native launch or file dialog",
      nativeAcceptance: false,
      calls: calls.map((call) => ({ ...call })),
      pending: [...pending.keys()],
      listenCalls,
      listeners: Object.fromEntries(
        [...listeners].map(([name, ids]) => [name, ids.size])
      ),
    }),
  };
}
