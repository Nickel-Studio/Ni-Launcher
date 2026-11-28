import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  Button,
  IconButton,
  IconRefresh,
  LoadingCard,
  Select,
  SegmentedControl,
  Slider,
  Switch,
  Tooltip,
} from "../ui";
import {
  GAME_MEMORY_MIN_MB,
  GAME_MEMORY_MAX_MB,
  loadGameSettings,
  saveGameSettings,
  type GameSettings,
} from "../version/gameSettings";
import {
  detectJava,
  describeError,
  resolveMemoryPlan,
  MEMORY_STEP_MB,
  type JavaInstallation,
  type MemoryPlanInfo,
} from "../version/vsettings/api";
import { useLaunchInstance } from "../../lib/instances";
import { GameMemory } from "./GameMemory";
import { OfflineSkinPanel } from "./OfflineSkinPanel";
import { loadGameRuntimeInfo, type GameRuntimeInfo } from "./gameRuntime";
import { SettingRow } from "./SettingRow";
import { WINDOW_TITLE_HELP } from "./SettingHelp";
import { SettingsGroup } from "./SettingsGroup";

const VISIBILITY_OPTIONS = [
  { value: "close", label: "游戏启动后立即关闭" },
  { value: "hide-close", label: "游戏启动后隐藏，游戏退出后自动关闭" },
  { value: "hide", label: "游戏启动后隐藏，游戏退出后重新打开" },
  { value: "minimize", label: "游戏启动后最小化" },
  { value: "keep", label: "游戏启动后仍保持不变" },
];
const PRIORITY_OPTIONS = [
  {
    value: "high",
    label: "高（优先保证游戏运行，游戏性能更佳，但可能造成其他程序卡顿）",
  },
  { value: "normal", label: "中（平衡）" },
  {
    value: "low",
    label: "低（优先保证其他程序运行，但可能让游戏卡顿，适合挂机时使用）",
  },
];
const WINDOW_OPTIONS = [
  { value: "fullscreen", label: "全屏" },
  { value: "default", label: "默认" },
  { value: "same-as-launcher", label: "与启动器尺寸一致" },
  { value: "custom", label: "自定义尺寸" },
  { value: "maximize", label: "最大化" },
];
const MEMORY_OPTIONS = [
  { value: "auto", label: "自动配置" },
  { value: "custom", label: "自定义" },
];
const MEMORY_MANAGEMENT_OPTIONS = [
  { value: "auto-zgc", label: "尽量使用 ZGC" },
  { value: "generational-zgc", label: "尽量使用分代 ZGC" },
  { value: "g1", label: "标准 G1GC" },
  { value: "g1-tuned", label: "调优 G1GC" },
  { value: "custom", label: "不指定（可自定义）" },
];
const VERSION_ISOLATION_HELP =
  "当安装新版本时，据此自动设置新版本的隔离选项。若想调整已有版本，请前往它的版本设置。";
const CUSTOM_INFO_HELP =
  "显示在游戏主界面的左下角与 F3 调试页面的左上角；支持 {date}、{time} 等替换标记。";
const MEMORY_MODE_HELP =
  "自动配置会根据安装的 Mod 量与电脑剩余内存动态调整；自定义使用下方滑块的值。";
const JVM_HELP =
  "启动 Minecraft 时使用的额外 JVM 参数，除非有确定把握，否则请不要修改。";
const GAME_ARGS_HELP =
  "内容会直接拼合在启动参数末尾，例如输入 --demo 以试玩模式启动。";
const PRE_LAUNCH_HELP = "在游戏启动前执行命令，语法与 Windows 命令提示符一致。";
const JAVA_LIST_HELP =
  "启动游戏时优先选择列表中第一个兼容当前版本的 Java；版本独立设置可强制指定。";
const LAUNCH_WRAPPER_HELP =
  "是否使用 Java Launch Wrapper 修复 Java 18- 在中文路径下可能无法正常启动的问题。";
const LWJGL_AGENT_HELP =
  "是否使用 LWJGL Unsafe Agent 修复 LWJGL 3.4.1 的一个性能问题。";
const HIGH_PERFORMANCE_GPU_HELP =
  "自动在 Windows 设置中将游戏和 Java 改为使用高性能显卡，避免误用集成显卡导致帧率偏低。";
const GC_HELP =
  "自动添加 GC 参数；只有选择“不指定”时，才能在 Java 虚拟机参数中手动设置。";
const WINDOW_MIN_WIDTH = 320;
const WINDOW_MIN_HEIGHT = 240;
const WINDOW_MAX_SIZE = 16384;

function WindowDimension({
  label,
  value,
  min,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  onChange: (value: number) => void;
}) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const number = Number(draft);
  const valid =
    draft !== "" &&
    Number.isInteger(number) &&
    number >= min &&
    number <= WINDOW_MAX_SIZE;
  return (
    <input
      className="st-input"
      type="number"
      min={min}
      max={WINDOW_MAX_SIZE}
      step={1}
      aria-label={label}
      aria-invalid={!valid}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={() => {
        if (valid) onChange(number);
        else setDraft(String(value));
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter" && valid) onChange(number);
        if (event.key === "Escape") setDraft(String(value));
      }}
    />
  );
}

/** 全局游戏设置与独立实例设置共用 game.json 的串行存取契约。 */
export function GamePanel() {
  const { instance } = useLaunchInstance();
  const [settings, setSettings] = useState<GameSettings | null>(null);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [javas, setJavas] = useState<JavaInstallation[]>([]);
  const [detecting, setDetecting] = useState(false);
  const [javaError, setJavaError] = useState("");
  const [runtime, setRuntime] = useState<GameRuntimeInfo | null>(null);
  const [memoryPlan, setMemoryPlan] = useState<MemoryPlanInfo | null>(null);
  const [memoryPlanError, setMemoryPlanError] = useState("");
  const alive = useRef(true);
  const current = useRef<GameSettings | null>(null);
  const sequence = useRef(0);
  const detectionBusy = useRef(false);
  const refreshJava = useCallback(async () => {
    if (detectionBusy.current) return;
    detectionBusy.current = true;
    setDetecting(true);
    setJavaError("");
    try {
      const found = await detectJava();
      if (alive.current) setJavas(found);
    } catch (reason) {
      if (alive.current)
        setJavaError(
          `无法读取 Java 列表：${describeError(
            reason
          )}。请重新扫描或填写 Java 路径。`
        );
    } finally {
      detectionBusy.current = false;
      if (alive.current) setDetecting(false);
    }
  }, []);

  const refreshRuntime = useCallback(async () => {
    try {
      const info = await loadGameRuntimeInfo();
      if (alive.current) setRuntime(info);
    } catch {
      // 运行信息错误由 GameMemory 行内提示；这里只保留平台能力查询。
      if (alive.current) setRuntime(null);
    }
  }, []);

  const refreshMemoryPlan = useCallback(async () => {
    setMemoryPlanError("");
    try {
      const plan = await resolveMemoryPlan(
        instance?.root ?? null,
        instance?.versionId ?? null
      );
      if (alive.current) setMemoryPlan(plan);
    } catch (reason) {
      if (alive.current) {
        setMemoryPlan(null);
        setMemoryPlanError(`无法读取内存规划：${describeError(reason)}`);
      }
    }
  }, [instance?.root, instance?.versionId]);

  useEffect(() => {
    alive.current = true;
    let cancelled = false;
    setError("");
    void loadGameSettings()
      .then((loaded) => {
        if (cancelled) return;
        current.current = loaded;
        setSettings(loaded);
      })
      .catch((reason) => {
        if (!cancelled) setError(`读取游戏设置失败：${describeError(reason)}`);
      });
    void refreshJava();
    void refreshRuntime();
    return () => {
      cancelled = true;
      alive.current = false;
    };
  }, [attempt, refreshJava, refreshRuntime]);

  useEffect(() => {
    void refreshMemoryPlan();
  }, [refreshMemoryPlan]);

  function persist(next: GameSettings) {
    const revision = ++sequence.current;
    setSaving(true);
    setError("");
    // 立即进入共享队列，让切换页面后的读取也等待所有已提交编辑。
    void saveGameSettings(next)
      .then(() => {
        if (alive.current && revision === sequence.current) setSaving(false);
      })
      .catch((reason) => {
        if (alive.current && revision === sequence.current) {
          setSaving(false);
          setError(
            `保存游戏设置失败：${describeError(reason)}。请检查配置目录后重试。`
          );
        }
      });
  }

  function patch(value: Partial<GameSettings>) {
    if (!current.current) return;
    const next = { ...current.current, ...value };
    current.current = next;
    setSettings(next);
    persist(next);
  }

  if (!settings)
    return error ? (
      <div className="st-game">
        <p className="st-save-error" role="alert">
          {error}
        </p>
        <Button onClick={() => setAttempt((value) => value + 1)}>
          重新读取
        </Button>
      </div>
    ) : (
      <LoadingCard label="正在载入游戏设置…" />
    );
  function row(
    label: string,
    control: ReactNode,
    help?: string,
    hint?: string
  ) {
    return (
      <SettingRow
        label={label}
        help={help}
        description={hint}
        fill
        control={control}
      />
    );
  }
  const javaOptions = [
    { value: "", label: "自动选择", tooltip: JAVA_LIST_HELP },
    ...javas.map((java) => ({
      value: java.path,
      label: `${java.version} · ${java.vendor || java.path}`,
      tooltip: java.path,
    })),
  ];
  if (
    settings.javaPath &&
    !javas.some((java) => java.path === settings.javaPath)
  ) {
    javaOptions.push({
      value: settings.javaPath,
      label: settings.javaPath,
      tooltip: settings.javaPath,
    });
  }
  const suggestedMemoryMb = memoryPlan?.suggestedMemoryMb ?? null;
  const allocatedMemoryMb =
    settings.memoryMode === "custom" ? settings.memoryMb : suggestedMemoryMb;
  const memoryManagement = MEMORY_MANAGEMENT_OPTIONS.some(
    (option) => option.value === settings.memoryManagement
  )
    ? String(settings.memoryManagement)
    : "custom";
  const manualMemoryMax = Math.max(
    GAME_MEMORY_MIN_MB,
    runtime?.totalMemoryMb ?? GAME_MEMORY_MAX_MB
  );
  // 只有后端明确返回 false 才禁用；能力未知时不阻塞用户操作。
  const capabilityUnavailable = (key: string) =>
    runtime?.capabilities[key] === false;
  const capabilityHint = (key: string): string | undefined =>
    runtime?.capabilities[key] === false
      ? "当前系统不支持此项，请使用默认设置。"
      : undefined;

  return (
    <div className="st-game">
      {error ? (
        <div className="st-game__feedback" role="alert">
          <p>{error}</p>
          <Button
            size="sm"
            onClick={() => current.current && persist(current.current)}
          >
            重试保存
          </Button>
        </div>
      ) : null}
      <SettingsGroup id="game-launch" title="启动选项">
        {row(
          "默认版本隔离",
          <Select
            ariaLabel="默认版本隔离"
            value={settings.versionIsolation ? "isolated" : "shared"}
            options={[
              { value: "shared", label: "关闭" },
              { value: "isolated", label: "隔离所有版本" },
            ]}
            onChange={(value) =>
              patch({ versionIsolation: value === "isolated" })
            }
          />,
          VERSION_ISOLATION_HELP
        )}
        {row(
          "游戏窗口标题",
          <input
            className="st-input"
            aria-label="游戏窗口标题"
            placeholder="默认"
            value={settings.windowTitle}
            disabled={capabilityUnavailable("windowTitle")}
            onChange={(event) => patch({ windowTitle: event.target.value })}
          />,
          WINDOW_TITLE_HELP,
          capabilityHint("windowTitle")
        )}
        {row(
          "自定义信息",
          <input
            className="st-input"
            aria-label="自定义信息"
            placeholder="默认"
            value={settings.customInfo}
            onChange={(event) => patch({ customInfo: event.target.value })}
          />,
          CUSTOM_INFO_HELP
        )}
        {row(
          "启动器可见性",
          <Select
            ariaLabel="启动器可见性"
            options={VISIBILITY_OPTIONS}
            value={settings.launcherVisibility}
            onChange={(value) =>
              patch({
                launcherVisibility: value as GameSettings["launcherVisibility"],
              })
            }
          />
        )}
        {row(
          "进程优先级",
          <Select
            ariaLabel="进程优先级"
            options={PRIORITY_OPTIONS.map((option) => ({
              ...option,
              disabled:
                option.value !== "normal" &&
                capabilityUnavailable("processPriority"),
            }))}
            value={settings.processPriority}
            onChange={(value) =>
              patch({
                processPriority: value as GameSettings["processPriority"],
              })
            }
          />,
          undefined,
          capabilityHint("processPriority")
        )}
        {row(
          "窗口大小",
          <Select
            ariaLabel="窗口大小"
            options={WINDOW_OPTIONS}
            value={settings.windowMode}
            onChange={(value) =>
              patch({ windowMode: value as GameSettings["windowMode"] })
            }
          />
        )}
        {settings.windowMode === "custom"
          ? row(
              "窗口尺寸",
              <div className="st-game__dimensions">
                <WindowDimension
                  label="窗口宽度"
                  min={WINDOW_MIN_WIDTH}
                  value={settings.windowWidth}
                  onChange={(value) => patch({ windowWidth: value })}
                />
                <span aria-hidden="true">×</span>
                <WindowDimension
                  label="窗口高度"
                  min={WINDOW_MIN_HEIGHT}
                  value={settings.windowHeight}
                  onChange={(value) => patch({ windowHeight: value })}
                />
              </div>
            )
          : null}
      </SettingsGroup>
      <SettingsGroup id="game-memory" title="内存分配">
        {row(
          "分配方式",
          <SegmentedControl
            options={MEMORY_OPTIONS}
            value={settings.memoryMode}
            onChange={(value) =>
              patch({ memoryMode: value as GameSettings["memoryMode"] })
            }
          />,
          MEMORY_MODE_HELP
        )}
        {row(
          "游戏内存",
          <Slider
            aria-label="游戏内存"
            min={GAME_MEMORY_MIN_MB}
            max={manualMemoryMax}
            step={MEMORY_STEP_MB}
            value={settings.memoryMb}
            suffix=" MB"
            disabled={settings.memoryMode !== "custom"}
            onChange={(value) => patch({ memoryMb: value })}
          />
        )}
        <GameMemory
          totalMemoryMb={runtime?.totalMemoryMb ?? null}
          availableMemoryMb={runtime?.availableMemoryMb ?? null}
          suggestedMemoryMb={suggestedMemoryMb}
          allocatedMemoryMb={allocatedMemoryMb}
        />
        {memoryPlanError ? (
          <p className="st-game-memory__error" role="alert">
            {memoryPlanError}
          </p>
        ) : null}
      </SettingsGroup>
      <OfflineSkinPanel
        settings={settings}
        onChange={patch}
        disabled={capabilityUnavailable("offlineSkin")}
      />
      <SettingsGroup id="game-advanced" title="高级选项" collapsible>
        {row(
          "Java 虚拟机参数",
          <textarea
            className="st-input st-game__textarea"
            aria-label="Java 虚拟机参数"
            rows={3}
            spellCheck={false}
            value={settings.jvmArgs}
            onChange={(event) => patch({ jvmArgs: event.target.value })}
          />,
          JVM_HELP
        )}
        {row(
          "游戏参数",
          <input
            className="st-input"
            aria-label="游戏参数"
            spellCheck={false}
            value={settings.gameArgs}
            onChange={(event) => patch({ gameArgs: event.target.value })}
          />,
          GAME_ARGS_HELP
        )}
        {row(
          "启动前执行命令",
          <input
            className="st-input"
            aria-label="启动前执行命令"
            spellCheck={false}
            value={settings.preLaunchCommand}
            onChange={(event) =>
              patch({ preLaunchCommand: event.target.value })
            }
          />,
          PRE_LAUNCH_HELP
        )}
        {row(
          "Java 列表",
          <div className="st-game__java">
            <Select
              ariaLabel="Java 列表"
              value={settings.javaPath}
              options={javaOptions}
              onChange={(value) => patch({ javaPath: value })}
            />
            <IconButton
              label="重新扫描 Java"
              disabled={detecting}
              onClick={() => void refreshJava()}
            >
              <IconRefresh />
            </IconButton>
          </div>
        )}

        {javaError ? (
          <p className="st-error" role="alert">
            {javaError}
          </p>
        ) : null}
        {row(
          "内存管理",
          <Select
            ariaLabel="内存管理"
            options={MEMORY_MANAGEMENT_OPTIONS}
            value={memoryManagement}
            onChange={(value) =>
              patch({
                memoryManagement: value as GameSettings["memoryManagement"],
              })
            }
          />,
          GC_HELP
        )}
        <div className="st-game__toggles">
          <Tooltip label={LAUNCH_WRAPPER_HELP}>
            <Switch
              label="禁用 Java Launch Wrapper"
              description={capabilityHint("launchWrapper")}
              checked={settings.disableWrapper === true}
              disabled={
                capabilityUnavailable("launchWrapper") &&
                settings.disableWrapper !== true
              }
              onChange={(value) => patch({ disableWrapper: value })}
            />
          </Tooltip>
          <Tooltip label={LWJGL_AGENT_HELP}>
            <Switch
              label="禁用 LWJGL Unsafe Agent"
              description={capabilityHint("lwjglAgent")}
              checked={settings.disableLwjglAgent === true}
              disabled={
                capabilityUnavailable("lwjglAgent") &&
                settings.disableLwjglAgent !== true
              }
              onChange={(value) => patch({ disableLwjglAgent: value })}
            />
          </Tooltip>
          <Tooltip label={HIGH_PERFORMANCE_GPU_HELP}>
            <Switch
              label="使用高性能显卡"
              description={capabilityHint("highPerformanceGpu")}
              checked={settings.highPerformanceGpu === true}
              disabled={
                capabilityUnavailable("highPerformanceGpu") &&
                settings.highPerformanceGpu !== true
              }
              onChange={(value) => patch({ highPerformanceGpu: value })}
            />
          </Tooltip>
        </div>
        <p className="st-game__choice-note">
          版本独立设置中还有更多高级选项可供调整。
        </p>
      </SettingsGroup>
      <p className="st-game__status" role="status">
        {saving ? "正在保存…" : ""}
      </p>
    </div>
  );
}
