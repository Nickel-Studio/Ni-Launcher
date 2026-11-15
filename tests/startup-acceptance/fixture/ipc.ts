import { mockIPC } from "@tauri-apps/api/mocks";
import { PRODUCT_VERSION } from "../../../src/lib/brand";
import type { OnboardingStatus } from "../../../src/lib/onboarding";

export interface ConsentRecord {
  eulaVersion?: string;
  eulaAcceptedAt?: number;
  changelogSeenVersion?: string;
}

interface ResponseOverride {
  value?: unknown;
  error?: string;
  delayMs?: number;
}

export interface Setup {
  mode?: "gate" | "app";
  transport?: "ipc" | "preview";
  record?: ConsentRecord;
  previewRecord?: unknown;
  previewRaw?: string;
  denyPreviewStorage?: "get" | "set";
  delays?: Record<string, number>;
  failures?: Record<string, number>;
  responses?: Record<string, ResponseOverride[]>;
  exiting?: boolean;
  portablePending?: boolean;
  autoLaunch?: boolean;
  holdCommands?: string[];
}

interface Event {
  kind: string;
  at: number;
  command?: string;
  payload?: Record<string, unknown>;
  outcome?: string;
}

const RECORD_KEY = "startup-acceptance-ipc-record";
const INITIALIZED_KEY = "startup-acceptance-initialized";
const PREVIEW_KEY = "nickel-onboarding-preview";
const EULA_REVISION = "minecraft-eula-v1";

export function installBackend(setup: Setup) {
  if (localStorage.getItem(INITIALIZED_KEY) === null) {
    if (setup.record)
      localStorage.setItem(RECORD_KEY, JSON.stringify(setup.record));
    if (setup.previewRaw !== undefined)
      localStorage.setItem(PREVIEW_KEY, setup.previewRaw);
    else if (setup.previewRecord !== undefined)
      localStorage.setItem(PREVIEW_KEY, JSON.stringify(setup.previewRecord));
    localStorage.setItem(INITIALIZED_KEY, "1");
  }
  let exiting = setup.exiting ?? false;
  let exitCount = 0;
  let autoLaunchCount = 0;
  let dispatched = false;
  const held = new Map<string, (() => void)[]>();
  const released = new Set<string>();
  const events: Event[] = [];
  const failures = { ...setup.failures };
  const responses = Object.fromEntries(
    Object.entries(setup.responses ?? {}).map(([key, value]) => [
      key,
      [...value],
    ])
  );

  const record = (): ConsentRecord =>
    JSON.parse(localStorage.getItem(RECORD_KEY) || "{}");
  const accepted = (value: ConsentRecord) =>
    value.eulaVersion === EULA_REVISION &&
    Number.isSafeInteger(value.eulaAcceptedAt) &&
    (value.eulaAcceptedAt ?? 0) > 0;
  const status = (): OnboardingStatus => {
    const value = record();
    return {
      eulaVersion: EULA_REVISION,
      eulaUrl: "https://www.minecraft.net/zh-hans/eula",
      eulaAcceptedAt: value.eulaAcceptedAt ?? null,
      currentVersion: PRODUCT_VERSION,
      needsEula: !accepted(value),
      needsChangelog: value.changelogSeenVersion !== PRODUCT_VERSION,
      exiting,
    };
  };
  const event = (kind: string) => events.push({ kind, at: performance.now() });
  const save = (value: ConsentRecord) => {
    localStorage.setItem(RECORD_KEY, JSON.stringify(value));
    event("simulated-record-write");
  };
  const wait = (milliseconds: number) =>
    new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));

  async function invoke(command: string, raw: unknown): Promise<unknown> {
    const payload =
      raw && typeof raw === "object" && !Array.isArray(raw)
        ? (raw as Record<string, unknown>)
        : {};
    const entry: Event = { kind: "ipc", command, at: performance.now() };
    if (command.startsWith("onboarding_")) entry.payload = payload;
    events.push(entry);
    if (command === "onboarding_reject" && !exiting) {
      exiting = true;
      event("simulated-backend-exit-scheduled");
      window.setTimeout(() => {
        exitCount += 1;
        event("simulated-backend-exit");
      }, 5000);
    }
    const override = responses[command]?.shift();
    if (setup.holdCommands?.includes(command) && !released.has(command)) {
      await new Promise<void>((resolve) => {
        held.set(command, [...(held.get(command) ?? []), resolve]);
      });
    }
    await wait(override?.delayMs ?? setup.delays?.[command] ?? 0);
    try {
      if (override?.error) throw new Error(override.error);
      if (override && "value" in override) {
        entry.outcome = "override";
        return override.value;
      }
      if ((failures[command] ?? 0) > 0) {
        failures[command] -= 1;
        throw new Error("Simulated local persistence failure; retry is safe.");
      }
      let value: unknown;
      switch (command) {
        case "onboarding_get":
          value = status();
          break;
        case "onboarding_accept_eula": {
          if (
            !payload.checked ||
            payload.eulaVersion !== EULA_REVISION ||
            exiting
          )
            throw new Error("Consent validation rejected.");
          const stored = record();
          if (!accepted(stored))
            save({
              ...stored,
              eulaVersion: EULA_REVISION,
              eulaAcceptedAt: Math.floor(Date.now() / 1000),
            });
          value = status();
          break;
        }
        case "onboarding_ack_changelog": {
          const stored = record();
          if (exiting || !accepted(stored))
            throw new Error(
              "Consent is required before release acknowledgment."
            );
          if (payload.version !== PRODUCT_VERSION)
            throw new Error("Release version does not match.");
          save({ ...stored, changelogSeenVersion: PRODUCT_VERSION });
          value = status();
          break;
        }
        case "onboarding_finish": {
          const current = status();
          if (current.needsEula || current.needsChangelog || exiting)
            throw new Error("Startup is still gated.");
          if (!dispatched) {
            dispatched = true;
            event("simulated-startup-dispatch");
            if (setup.autoLaunch) autoLaunchCount += 1;
          }
          value = null;
          break;
        }
        case "onboarding_reject":
          value = null;
          break;
        case "window_close":
          event("simulated-window-close-request");
          value = null;
          break;
        case "portable_bootstrap_status":
          value = setup.portablePending
            ? {
                packageId: "fixture-package",
                name: "Acceptance Fixture",
                gameDirectory: "C:/startup-fixture/.minecraft",
                state: "pending",
                taskId: null,
                versionId: null,
              }
            : null;
          break;
        case "portable_bootstrap_start":
          value = "fixture-import-task";
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
        case "settings_get":
          value = "{}";
          break;
        case "settings_set":
          value = null;
          break;
        case "accounts_list":
          value = [];
          break;
        case "account_active":
          value = null;
          break;
        case "instances_list":
          value = [];
          break;
        case "game_get":
          value = { gameDir: "C:/startup-fixture/.minecraft", version: null };
          break;
        case "installed_versions":
          value = [];
          break;
        case "active_account_get":
          value = null;
          break;
        default:
          if (command.startsWith("plugin:opener|")) value = null;
          else if (command.startsWith("plugin:")) value = null;
          else throw new Error("Unconfigured fixture command: " + command);
      }
      entry.outcome = "resolved";
      return value;
    } catch (cause) {
      entry.outcome = "rejected";
      throw cause;
    }
  }

  if (setup.transport !== "preview")
    mockIPC(invoke, { shouldMockEvents: true });
  if (setup.denyPreviewStorage) {
    const operation =
      setup.denyPreviewStorage === "set" ? "setItem" : "getItem";
    const original = Storage.prototype[operation];
    Object.defineProperty(Storage.prototype, operation, {
      configurable: true,
      value(this: Storage, key: string, ...args: string[]) {
        if (key === PREVIEW_KEY)
          throw new DOMException("Fixture storage denied", "SecurityError");
        return Reflect.apply(original, this, [key, ...args]);
      },
    });
  }
  return {
    setup,
    events,
    event,
    release(command: string) {
      released.add(command);
      held.get(command)?.forEach((resolve) => resolve());
      held.delete(command);
    },
    snapshot: () => ({
      scope: "headless-browser-production-components-with-simulated-ipc",
      nativeAcceptance: false,
      record: record(),
      previewRecord: localStorage.getItem(PREVIEW_KEY),
      exiting,
      exitCount,
      autoLaunchCount,
      events: [...events],
    }),
  };
}
