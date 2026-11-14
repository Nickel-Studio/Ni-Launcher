/* 浏览器回归仅隔离 IPC；启动画面、外壳、Pager、加载动画和安装按钮均使用真实组件。 */
(() => {
  const config = window.__startupConfig;
  const state = { calls: [], times: {}, pendingProviders: 0 };
  // hidden 场景：窗口在页面加载期间就不可见，直到 600ms 后才真正显示。
  // 闪屏必须保持到真实可见之后再淡出，不能因为 document.hidden 提前删除。
  let startupHidden = config.splash === "hidden";
  Object.defineProperty(document, "hidden", {
    configurable: true,
    get: () => startupHidden,
  });
  Object.defineProperty(document, "visibilityState", {
    configurable: true,
    get: () => (startupHidden ? "hidden" : "visible"),
  });
  if (config.splash === "hidden") {
    window.setTimeout(() => {
      startupHidden = false;
      // 记录窗口转为可见的时刻，供回归断言「放行不早于可见」。
      state.times.shown = performance.now();
      document.dispatchEvent(new Event("visibilitychange"));
    }, 600);
  }
  window.__startupRegression = state;
  // SplashScreen 通过给 document.body 加 inert 冻结整个应用（覆盖 createPortal 到
  // body 的 Modal / Tooltip / Toast）。在原型层记录启动期的 inert 写入，避免极短
  // 路径下 MutationObserver 错过「加上又立刻移除」的窗口；交接后的实际属性仍直接
  // 读取真实 DOM。替身只依赖 body / #root 的 setAttribute / removeAttribute。
  const nativeSetAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (name, value) {
    if (
      name === "inert" &&
      (this === document.body || this.id === "root") &&
      state.times.ready === undefined &&
      document.getElementById("nw-splash")
    ) {
      state.appInertDuringStartup = true;
    }
    return nativeSetAttribute.call(this, name, value);
  };
  const rootDirectory = "C:/NoWheRE-startup-regression";
  const callbacks = new Map();
  let callbackId = 0;
  const settings = { themeMode: config.mode, accent: "mono" };
  localStorage.setItem("nowhere.settings", JSON.stringify(settings));
  const version = {
    id: "1.14.3",
    type: "release",
    releaseTime: "2019-06-03T00:00:00Z",
  };
  const provider = (loader) => {
    const builds =
      config.catalog === "long" && loader === "forge"
        ? Array.from({ length: 40 }, (_, index) => ({
            version: `1.14.3-27.0.${index + 1}`,
            label: `27.0.${index + 1}`,
            releaseTime: version.releaseTime,
            capability: { install: true, saveInstaller: false },
          }))
        : [];
    return {
      loader,
      builds,
      provider: {
        state:
          config.catalog === "short"
            ? "hidden"
            : builds.length
            ? "available"
            : "unavailable",
        count: builds.length,
        capability: { install: builds.length > 0, saveInstaller: false },
        error: "",
      },
      cached: false,
      elapsedMs: 0,
    };
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    // @tauri-apps/api 的 getCurrentWindow() 会同步读取该字段；缺失会同步抛错。
    metadata: { currentWindow: { label: "main" } },
    async invoke(command, args = {}) {
      state.calls.push({ command, args });
      // 窗口 API 替身：hidden 场景在 600ms 前返回不可见，之后返回可见。
      // stuck 场景模拟窗口 API 永久无响应：验证 1500ms 查询超时不会断链，
      // 以及 6 秒硬截止最终必须交接外壳并移除闪屏。
      if (command === "plugin:window|is_visible") {
        if (config.splash === "stuck") return new Promise(() => {});
        return !startupHidden;
      }
      if (command === "settings_get") {
        if (config.settings === "pending") return new Promise(() => {});
        if (config.settings === "failed")
          throw new Error("fixture settings read failed");
        return JSON.stringify(settings);
      }
      if (command === "accounts_list" || command === "app_tasks_snapshot")
        return [];
      if (command === "instances_get") {
        return JSON.stringify({
          roots: [rootDirectory],
          active: rootDirectory,
        });
      }
      if (command === "instance_scan") return "[]";
      if (command === "list_remote_versions") {
        if (config.manifest === "pending") return new Promise(() => {});
        return JSON.stringify({
          latest: { release: version.id, snapshot: "" },
          versions: [version],
        });
      }
      if (command === "list_loader_provider") {
        if (config.catalog === "pending") {
          state.pendingProviders += 1;
          return new Promise(() => {});
        }
        return JSON.stringify(provider(args.loader));
      }
      if (command === "list_loaders") {
        const ids = ["forge", "neoforge", "fabric", "optifine", "liteloader"];
        return JSON.stringify({
          ...Object.fromEntries(ids.map((id) => [id, provider(id).builds])),
          providers: Object.fromEntries(
            ids.map((id) => [id, provider(id).provider])
          ),
        });
      }
      if (command === "install_version") return "startup-regression-task";
      if (command === "plugin:event|listen") return ++callbackId;
      return null;
    },
    transformCallback(callback) {
      const id = ++callbackId;
      callbacks.set(id, callback);
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    convertFileSrc(value) {
      return value;
    },
  };

  document.addEventListener(
    "transitionend",
    (event) => {
      if (event.target.id !== "nw-splash" || event.propertyName !== "opacity")
        return;
      state.times.transitionEnd = performance.now();
      if (config.splash === "missing-event") event.stopImmediatePropagation();
    },
    true
  );
  document.addEventListener(
    "transitioncancel",
    (event) => {
      if (
        event.target.id === "nw-splash" &&
        config.splash === "missing-event"
      ) {
        event.stopImmediatePropagation();
      }
    },
    true
  );
  const observer = new MutationObserver(() => {
    const splash = document.getElementById("nw-splash");
    // MutationObserver 兜底记录启动期 inert（Element.prototype 替身之外的路径）。
    if (
      state.times.ready === undefined &&
      splash &&
      ((document.body && document.body.hasAttribute("inert")) ||
        (document.getElementById("root") &&
          document.getElementById("root").hasAttribute("inert")))
    ) {
      state.appInertDuringStartup = true;
    }
    if (
      splash &&
      config.splash === "no-transition" &&
      !splash.style.transition
    ) {
      splash.style.setProperty("transition", "none", "important");
    }
    if (
      document.querySelector(".nw-shell") &&
      state.times.commit === undefined
    ) {
      state.times.commit = performance.now();
    }
    if (
      document.documentElement.classList.contains("nw-app-ready") &&
      state.times.ready === undefined
    ) {
      state.times.ready = performance.now();
      if (splash && config.splash === "cancel") {
        splash.dispatchEvent(
          new TransitionEvent("transitioncancel", { propertyName: "opacity" })
        );
      }
      if (splash && config.splash === "child-transition") {
        splash.firstElementChild.dispatchEvent(
          new TransitionEvent("transitionend", {
            propertyName: "opacity",
            bubbles: true,
          })
        );
        splash.dispatchEvent(
          new TransitionEvent("transitionend", { propertyName: "transform" })
        );
        state.ignoredUnrelatedTransitions = splash.isConnected;
      }
    }
    // 记录淡出开始的时刻：硬截止场景据此断言闪屏不会提前消失。
    if (
      splash &&
      splash.classList.contains("nw-splash--leaving") &&
      state.times.leaving === undefined
    ) {
      state.times.leaving = performance.now();
    }
    if (
      state.times.ready !== undefined &&
      !document.getElementById("nw-splash") &&
      state.times.removed === undefined
    ) {
      state.times.removed = performance.now();
      // 交接后的应用可交互性：正常淡出 / 硬截止都必须移除 body / #root / 外壳的
      // inert，并恢复指针事件。启动遮挡现在惰性化 document.body，以覆盖 body 级 portal。
      const shell = document.querySelector(".nw-shell");
      const rootElement = document.getElementById("root");
      state.appAfterReveal = {
        bodyInert: Boolean(
          document.body && document.body.hasAttribute("inert")
        ),
        rootInert: Boolean(rootElement && rootElement.hasAttribute("inert")),
        shellInert: Boolean(shell && shell.hasAttribute("inert")),
        shellPointerEvents: shell
          ? getComputedStyle(shell).pointerEvents
          : null,
      };
      observer.disconnect();
    }
  });
  observer.observe(document, {
    subtree: true,
    childList: true,
    attributes: true,
  });
})();
