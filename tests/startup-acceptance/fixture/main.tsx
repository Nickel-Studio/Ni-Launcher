import { StrictMode, useCallback, useState } from "react";
import { createRoot } from "react-dom/client";
import { StartupGate } from "../../../src/components/startup/StartupGate";
import { PortableBootstrap } from "../../../src/components/PortableBootstrap";
import { Button } from "../../../src/components/ui/Button";
import * as onboarding from "../../../src/lib/onboarding";
import { installBackend, type Setup } from "./ipc";
import "../../../src/styles/tokens.css";
import "../../../src/styles/global.css";
import "../../../src/styles/ui.css";
import "../../../src/styles/shell.css";

declare global {
  interface Window {
    __STARTUP_SETUP__?: Setup;
    __startupFixture: {
      snapshot: () => Record<string, unknown>;
      hideGate: () => void;
      showGate: () => void;
      rerender: () => void;
      unmount: () => void;
      release: (command: string) => void;
      api: typeof onboarding;
    };
  }
}

const activeIntervals = new Set<number>();
const originalInterval = window.setInterval.bind(window);
const originalClear = window.clearInterval.bind(window);
window.setInterval = ((
  handler: TimerHandler,
  timeout?: number,
  ...args: unknown[]
) => {
  const id = originalInterval(handler, timeout, ...args);
  activeIntervals.add(id);
  return id;
}) as typeof window.setInterval;
window.clearInterval = (id?: number) => {
  if (id !== undefined) activeIntervals.delete(id);
  originalClear(id);
};

const trackedListeners = new Map<
  string,
  Set<EventListenerOrEventListenerObject>
>();
const originalAdd = document.addEventListener.bind(document);
const originalRemove = document.removeEventListener.bind(document);
document.addEventListener = ((
  type: string,
  listener: EventListenerOrEventListenerObject,
  options?: boolean | AddEventListenerOptions
) => {
  if (type === "keydown" || type === "focusin") {
    if (!trackedListeners.has(type)) trackedListeners.set(type, new Set());
    trackedListeners.get(type)!.add(listener);
  }
  originalAdd(type, listener, options);
}) as typeof document.addEventListener;
document.removeEventListener = ((
  type: string,
  listener: EventListenerOrEventListenerObject,
  options?: boolean | EventListenerOptions
) => {
  trackedListeners.get(type)?.delete(listener);
  originalRemove(type, listener, options);
}) as typeof document.removeEventListener;

const backend = installBackend(window.__STARTUP_SETUP__ ?? {});
const root = createRoot(document.getElementById("root")!);
let readyCount = 0;
let shellClicks = 0;
const readyGenerations: number[] = [];
let hideGate: () => void = () => undefined;
let showGate: () => void = () => undefined;
let rerender: () => void = () => undefined;

function GateFixture() {
  const [ready, setReady] = useState(false);
  const [show, setShow] = useState(true);
  const [generation, setGeneration] = useState(0);
  hideGate = () => setShow(false);
  showGate = () => setShow(true);
  rerender = () => setGeneration((value) => value + 1);
  const onReady = useCallback(() => {
    readyCount += 1;
    readyGenerations.push(generation);
    backend.event("on-ready");
    setReady(true);
  }, [generation]);
  document.documentElement.classList.add("ni-app-ready");
  return (
    <>
      <div className="ni-shell" data-testid="fixture-shell" inert={!ready}>
        <main className="ni-content">
          <h1>Nickel acceptance fixture</h1>
          <p>
            Production startup components; simulated IPC, not a native process.
          </p>
          <Button
            data-testid="shell-action"
            onClick={() => {
              shellClicks += 1;
            }}
          >
            Shell action
          </Button>
        </main>
      </div>
      {show ? <StartupGate onReady={onReady} /> : null}
      {ready ? <PortableBootstrap /> : null}
    </>
  );
}

window.__startupFixture = {
  snapshot: () => ({
    ...backend.snapshot(),
    readyCount,
    readyGenerations: [...readyGenerations],
    shellClicks,
    activeIntervals: activeIntervals.size,
    modalListeners: Object.fromEntries(
      [...trackedListeners].map(([type, listeners]) => [type, listeners.size])
    ),
    shellInert: document.querySelector<HTMLElement>(".ni-shell")?.inert ?? null,
    dialogs: document.querySelectorAll('[role="dialog"]').length,
  }),
  hideGate: () => hideGate(),
  showGate: () => showGate(),
  rerender: () => rerender(),
  unmount: () => root.unmount(),
  release: (command: string) => backend.release(command),
  api: onboarding,
};

if (backend.setup.mode === "app") {
  const { default: App } = await import("../../../src/App");
  root.render(
    <StrictMode>
      <App />
    </StrictMode>
  );
} else {
  root.render(
    <StrictMode>
      <GateFixture />
    </StrictMode>
  );
}
