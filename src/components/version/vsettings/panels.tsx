import { SkinAuthSettings } from "../../settings/SkinAuthSettings";
import { useSettings } from "../../../lib/settings";
/**
 * 版本设置页的两个右侧独立页：概览（个性化 / 快捷方式 / 高级管理）与设置
 * （启动选项 / 内存分配 / 高级选项）。
 *
 * 所有控件都对接真实命令并即时持久化：
 * - 概览：`open_folder` / `version_rename` / `version_complete_files` / `version_delete`
 * - 设置：`version_settings_get` / `version_settings_set` / `java_detect`
 * 写入失败时给出可执行的中文提示，绝不静默吞掉。
 */
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { setRoute } from "../../../lib/nav";
import { DEFAULT_SETTINGS as DEFAULT_APP_SETTINGS } from "../../../lib/settings";
import { GameMemory } from "../../settings/GameMemory";
import { WINDOW_TITLE_HELP } from "../../settings/SettingHelp";
import {
  loadGameRuntimeInfo,
  type GameRuntimeInfo,
} from "../../settings/gameRuntime";
import {
  Button,
  Card,
  Collapse,
  LoadingCard,
  SegmentedControl,
  Select,
  Slider,
  Tooltip,
} from "../../ui";
import { InheritedBoolean, InheritedText } from "./InheritedText";
import { modUpdatesSetting } from "./mods";
import {
  blockIconSrc,
  dirName,
  formatReleaseTime,
  loaderLabel,
  typeLabel,
} from "../instanceVisuals";
import { confirmDialog } from "../dialog";
import { loadGameSettings, type GameSettings } from "../gameSettings";
import {
  completeVersionFiles,
  deleteVersion,
  describeError,
  describeJavaSelection,
  fetchVersionDetail,
  fetchVersionSettings,
  normalizeSettings,
  renameVersion,
  resolveJava,
  resolveMemoryPlan,
  saveVersionSettings,
  MEMORY_MAX_MB,
  MEMORY_MIN_MB,
  MEMORY_STEP_MB,
  type JavaInstallation,
  type JavaSelection,
  type MemoryMode,
  type MemoryPlanInfo,
  type ServerLogin,
  type VersionDetail,
  type VersionSettings,
} from "./api";

/** 概览面板的数据（`version_detail` 归一化结果，字段见 api.ts）。 */
export type OverviewData = {
  id: string;
  dir: string;
  type: string;
  loader: string;
  loaderVersion: string | null;
  icon: string;
  name: string;
  description: string;
  favorite: boolean;
  category: string;
  hasJar: boolean;
  jarSizeBytes: number;
  releaseTime: string | null;
};

interface OverviewPanelProps {
  data: OverviewData;
  root: string;
  /** 只回传发生变化的字段，由父级原地合并，避免每次修改都整页重载。 */
  onChanged: (patch: Partial<OverviewData>) => void;
}

/** 快捷方式条目（打开游戏目录下对应文件夹）。 */
const SHORTCUTS: { which: string; label: string }[] = [
  { which: "version", label: "版本文件夹" },
  { which: "saves", label: "存档文件夹" },
  { which: "mods", label: "Mod 文件夹" },
  { which: "screenshots", label: "截图文件夹" },
];

/** 可选的 Blocks 图标（覆盖全部自动判定键 + 自动跟随）。 */
const ICON_CHOICES: { value: string; label: string; src: string | null }[] = [
  { value: "auto", label: "自动", src: null },
  { value: "grass", label: "原版", src: blockIconSrc("grass") },
  { value: "command_block", label: "快照", src: blockIconSrc("command_block") },
  { value: "cobblestone", label: "旧版", src: blockIconSrc("cobblestone") },
  { value: "anvil", label: "Forge", src: blockIconSrc("anvil") },
  { value: "neoforge", label: "NeoForge", src: blockIconSrc("neoforge") },
  { value: "fabric", label: "Fabric", src: blockIconSrc("fabric") },
  { value: "grass_path", label: "OptiFine", src: blockIconSrc("grass_path") },
  { value: "egg", label: "LiteLoader", src: blockIconSrc("egg") },
  { value: "gold_block", label: "金块", src: blockIconSrc("gold_block") },
  {
    value: "redstone_block",
    label: "红石块",
    src: blockIconSrc("redstone_block"),
  },
  {
    value: "redstone_lamp_on",
    label: "红石灯（开）",
    src: blockIconSrc("redstone_lamp_on"),
  },
  {
    value: "redstone_lamp_off",
    label: "红石灯（关）",
    src: blockIconSrc("redstone_lamp_off"),
  },
];

/** 分组卡片（与设置页 SettingsGroup 同一视觉语言，就近实现避免跨模块依赖）。 */
export function Group({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <Card
      className="vst-group"
      padding="md"
      role="region"
      aria-label={title}
      title={title}
      subtitle={description}
    >
      <div className="vst-group__body">{children}</div>
    </Card>
  );
}

/** 单行设置项。 */
export function Row({
  label,
  hint,
  help,
  children,
  stacked = false,
}: {
  label: ReactNode;
  hint?: ReactNode;
  help?: string;
  children: ReactNode;
  stacked?: boolean;
}) {
  return (
    <div className={`vst-row${stacked ? " vst-row--stacked" : ""}`}>
      <div className="vst-row__text">
        <div className="vst-row__label">{label}</div>
        {hint ? <div className="vst-row__hint">{hint}</div> : null}
      </div>
      <div className="vst-row__control">
        {help ? (
          <Tooltip label={help}>
            <div className="vst-row__control-inner">{children}</div>
          </Tooltip>
        ) : (
          children
        )}
      </div>
    </div>
  );
}

/** 人类可读的文件大小。 */
function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${
    value >= 100 || unit === 0 ? Math.round(value) : value.toFixed(1)
  } ${units[unit]}`;
}

// ─────────────────────────────────────────────────────────────────────────────
// 概览
// ─────────────────────────────────────────────────────────────────────────────

export function OverviewPanel({ data, root, onChanged }: OverviewPanelProps) {
  const displayName = data.name || data.id;
  // 编辑态：修改版本名 / 描述 / 分类（只有点「保存」才写盘）。
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(displayName);
  const [description, setDescription] = useState(data.description);
  const [category, setCategory] = useState(data.category);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [busyAction, setBusyAction] = useState("");

  // 图标选择持久化在 per-version 设置的扩展字段 `icon` 里。
  const [iconChoice, setIconChoice] = useState("auto");
  const [iconSaving, setIconSaving] = useState(false);
  const [scriptPath, setScriptPath] = useState("");
  useEffect(() => {
    let alive = true;
    void fetchVersionSettings(root, data.id)
      .then((settings) => {
        if (alive) setIconChoice(settings.icon ?? "auto");
      })
      .catch((err: unknown) => {
        if (alive) setError(describeError(err));
      });
    return () => {
      alive = false;
    };
  }, [root, data.id]);

  const shownIcon = blockIconSrc(
    iconChoice === "auto" ? data.icon : iconChoice
  );
  const iconOptions = [
    ...ICON_CHOICES,
    { value: "custom", label: "自定义图片…", src: null },
  ];
  if (
    iconChoice !== "auto" &&
    !ICON_CHOICES.some((choice) => choice.value === iconChoice)
  ) {
    iconOptions.push({
      value: iconChoice,
      label: "自定义图片",
      src: shownIcon,
    });
  }

  const startEdit = useCallback(() => {
    setName(data.name || data.id);
    setDescription(data.description);
    setCategory(data.category);
    setError("");
    setEditing(true);
  }, [data]);

  const saveEdit = useCallback(async () => {
    setSaving(true);
    setError("");
    try {
      await renameVersion(root, data.id, {
        name: name.trim(),
        description: description.trim(),
        category: category.trim(),
        favorite: data.favorite,
      });
      setEditing(false);
      onChanged({
        name: name.trim(),
        description: description.trim(),
        category: category.trim(),
      });
    } catch (err) {
      setError(describeError(err));
    } finally {
      setSaving(false);
    }
  }, [root, data, name, description, category, onChanged]);

  const toggleFavorite = useCallback(async () => {
    setSaving(true);
    setError("");
    try {
      await renameVersion(root, data.id, {
        name: data.name,
        description: data.description,
        category: data.category,
        favorite: !data.favorite,
      });
      onChanged({ favorite: !data.favorite });
    } catch (err) {
      setError(describeError(err));
    } finally {
      setSaving(false);
    }
  }, [root, data, onChanged]);

  const pickIcon = useCallback(
    async (value: string) => {
      if (iconSaving) return;
      setIconSaving(true);
      setError("");
      try {
        const icon =
          value === "custom"
            ? await invoke<string | null>("version_icon_pick", {
                dir: root,
                versionId: data.id,
              })
            : value === "auto"
            ? null
            : value;
        if (value === "custom") {
          if (icon === null) return;
          const image = new Image();
          image.src = blockIconSrc(icon);
          try {
            await image.decode();
          } catch {
            throw new Error("图标无法解码，请重新导出 PNG、JPEG 或 WebP 图片");
          }
        }
        const settings = await fetchVersionSettings(root, data.id);
        await saveVersionSettings(root, data.id, { ...settings, icon });
        // 卡片图标由本组件的 iconChoice 立即渲染，不需要再触发父级重载。
        setIconChoice(icon ?? "auto");
      } catch (err) {
        setError(describeError(err));
      } finally {
        setIconSaving(false);
      }
    },
    [root, data.id, iconSaving]
  );

  const openFolder = useCallback(
    async (which: string) => {
      setError("");
      try {
        await invoke("open_folder", { dir: root, which, versionId: data.id });
      } catch (err) {
        setError(describeError(err));
      }
    },
    [root, data.id]
  );

  const completeFiles = useCallback(async () => {
    setBusyAction("complete");
    setError("");
    try {
      await completeVersionFiles(root, data.id);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusyAction("");
    }
  }, [root, data.id]);

  const exportScript = useCallback(async () => {
    setBusyAction("script");
    setError("");
    setScriptPath("");
    try {
      const path = await invoke<string | null>("version_export_script", {
        dir: root,
        versionId: data.id,
      });
      if (path !== null) setScriptPath(path);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusyAction("");
    }
  }, [root, data.id]);

  const removeVersion = useCallback(async () => {
    setError("");
    setBusyAction("delete");
    try {
      const ok = await confirmDialog(
        `删除实例「${displayName}」及其目录内的全部文件？\n${data.dir}`,
        "删除实例"
      );
      if (!ok) return;
      await deleteVersion(root, data.id);
      setRoute("instances");
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusyAction("");
    }
  }, [root, data.id, data.dir, displayName]);

  return (
    <div className="vst-panel">
      {error ? (
        <p className="vst-alert" role="alert">
          {error}
        </p>
      ) : null}

      {/* 版本头：图标 + 名称 + 类型 / 加载器徽标 */}
      <div className="vst-head">
        <img className="vst-head__icon" src={shownIcon} alt="" />
        <div className="vst-head__text">
          <div className="vst-head__name">{displayName}</div>
          <div className="vst-head__badges">
            <span className="vst-badge">{typeLabel(data.type)}</span>
            <span className="vst-badge">
              {loaderLabel(data.loader)}
              {data.loaderVersion ? (
                <span className="vst-badge__ver">{data.loaderVersion}</span>
              ) : null}
            </span>
            {data.favorite ? (
              <span className="vst-badge vst-badge--fav">已收藏</span>
            ) : null}
          </div>
          {data.description ? (
            <p className="vst-head__desc">{data.description}</p>
          ) : null}
          <div className="vst-head__meta">
            {dirName(root)} ·{" "}
            {data.hasJar ? formatBytes(data.jarSizeBytes) : "缺少 jar"}
            {data.releaseTime
              ? ` · ${formatReleaseTime(data.releaseTime)}`
              : ""}
          </div>
        </div>
      </div>

      {/* 个性化 */}
      <Group title="个性化" description="只在启动器里生效的展示信息">
        <Row label="图标">
          <Select
            ariaLabel="实例图标"
            value={iconChoice}
            disabled={iconSaving}
            options={iconOptions}
            onChange={(value) => void pickIcon(value)}
          />
        </Row>
        <Row label="分类">
          <Select
            ariaLabel="实例分类"
            value={editing ? category : data.category}
            disabled={saving}
            options={[
              { value: "", label: "自动" },
              { value: "hidden", label: "从实例列表中隐藏" },
              { value: "modded", label: "可安装 Mod 的实例" },
              { value: "regular", label: "常规实例" },
              { value: "rare", label: "不常用实例" },
              { value: "april_fools", label: "愚人节版本" },
            ]}
            onChange={(value) => {
              if (editing) {
                setCategory(value);
                return;
              }
              const previous = data.category;
              setSaving(true);
              setError("");
              // 先原地更新，分类选择立即回流到卡片与徽标；失败再回滚。
              onChanged({ category: value });
              void renameVersion(root, data.id, {
                name: data.name,
                description: data.description,
                category: value,
                favorite: data.favorite,
              })
                .catch((err: unknown) => {
                  onChanged({ category: previous });
                  setError(describeError(err));
                })
                .finally(() => setSaving(false));
            }}
          />
        </Row>

        {/* 信息与卡片错峰：展开先撑高再淡入，收起先淡出再收拢。 */}
        <Collapse open={editing} staggered>
          <Row label="显示名称">
            <input
              className="vst-input"
              aria-label="显示名称"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={data.id}
              spellCheck={false}
            />
          </Row>
          <Row label="描述">
            <textarea
              className="vst-input vst-textarea"
              aria-label="描述"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={2}
              placeholder="给这个版本写一句话备注"
            />
          </Row>
        </Collapse>

        {editing ? (
          <div className="vst-actions">
            <Button
              variant="ghost"
              onClick={() => setEditing(false)}
              disabled={saving}
            >
              取消
            </Button>
            <Button
              variant="primary"
              loading={saving}
              onClick={() => void saveEdit()}
            >
              保存
            </Button>
          </div>
        ) : (
          <div className="vst-actions">
            <Button
              variant={data.favorite ? "secondary" : "ghost"}
              loading={saving}
              onClick={() => void toggleFavorite()}
            >
              {data.favorite ? "从收藏夹中移除" : "加入收藏夹"}
            </Button>
            <Button variant="secondary" onClick={startEdit}>
              修改信息
            </Button>
          </div>
        )}
      </Group>

      {/* 快捷方式 */}
      <Group title="快捷方式" description="在系统文件管理器中打开对应目录">
        <div className="vst-shortcuts">
          {SHORTCUTS.map((item) => (
            <Button
              key={item.which}
              variant="secondary"
              className="vst-shortcut"
              fullWidth
              onClick={() => void openFolder(item.which)}
            >
              {item.label}
            </Button>
          ))}
        </div>
      </Group>

      {/* 高级管理 */}
      <Group title="高级管理">
        <Row label="导出启动脚本" help="选择保存位置，生成当前实例的启动脚本">
          <Button
            variant="secondary"
            disabled={busyAction !== ""}
            loading={busyAction === "script"}
            onClick={() => void exportScript()}
          >
            导出启动脚本
          </Button>
        </Row>
        <Collapse open={Boolean(scriptPath)} staggered>
          {scriptPath ? (
            <Tooltip label={scriptPath}>
              <p className="vst-static" role="status" tabIndex={0}>
                启动脚本已保存：{scriptPath}
              </p>
            </Tooltip>
          ) : null}
        </Collapse>
        <Row
          label="补全文件"
          hint={
            data.hasJar
              ? "检查并补齐缺失或校验不符的游戏文件"
              : "当前缺少游戏 jar，补全后才能启动"
          }
        >
          <Button
            variant="secondary"
            disabled={busyAction !== ""}
            loading={busyAction === "complete"}
            onClick={() => void completeFiles()}
          >
            补全文件
          </Button>
        </Row>
        <Row label="删除版本" hint="删除版本文件夹内的全部内容">
          <Button
            variant="danger"
            disabled={busyAction !== ""}
            loading={busyAction === "delete"}
            onClick={() => void removeVersion()}
          >
            删除版本
          </Button>
        </Row>
      </Group>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// 设置
// ─────────────────────────────────────────────────────────────────────────────

interface SettingsPanelProps {
  root: string;
  versionId: string;
  javas: JavaInstallation[];
}

const MEMORY_MODE_OPTIONS: { value: MemoryMode; label: string }[] = [
  { value: "global", label: "跟随全局" },
  { value: "auto", label: "自动" },
  { value: "custom", label: "自定义" },
];

const MEMORY_MANAGEMENT_OPTIONS: {
  value: GameSettings["memoryManagement"];
  label: string;
}[] = [
  { value: "auto-zgc", label: "尽量使用 ZGC" },
  { value: "generational-zgc", label: "尽量使用分代 ZGC" },
  { value: "g1", label: "标准 G1GC" },
  { value: "g1-tuned", label: "调优 G1GC" },
  { value: "custom", label: "不指定（可自定义）" },
];
const SERVER_LOGIN_OPTIONS: { value: ServerLogin; label: string }[] = [
  { value: "any", label: "正版登录或离线登录" },
  { value: "microsoft", label: "仅正版登录" },
  { value: "offline", label: "仅离线登录" },
  { value: "thirdParty", label: "第三方登录：Authlib Injector 或 LittleSkin" },
];
const LITTLESKIN_PRESET: Pick<
  VersionSettings,
  "serverLogin" | "authServer" | "serverRegistration" | "serverName"
> = {
  serverLogin: "thirdParty",
  authServer: new URL("/api/yggdrasil", DEFAULT_APP_SETTINGS.littleskinServer)
    .href,
  serverRegistration: new URL(
    "/auth/register",
    DEFAULT_APP_SETTINGS.littleskinServer
  ).href,
  serverName: "LittleSkin 登录",
};

/**
 * 实例独立设置的说明文案。
 *
 * 与全局设置一样，说明只挂在控件容器上，由 Tooltip 在悬浮 / 键盘聚焦时显示；
 * 标签保持普通文本，不改变光标、不加下划线。说明保留字段的真实作用与
 * 继承语义（null 跟随全局，空串显式清空）。
 */
const VERSION_ISOLATION_HELP =
  "开启后使用独立的存档、配置与资源目录，与其他版本互不影响；关闭后与其他未隔离版本共享这些文件。";
const CUSTOM_INFO_HELP =
  "显示在游戏主界面的左下角与 F3 调试页面的左上角；支持 {date}、{time} 等替换标记。";
const JAVA_HELP =
  "启动游戏时优先选择列表中第一个兼容当前版本的 Java；版本独立设置可强制指定。";
const MEMORY_MODE_HELP =
  "跟随全局使用全局设置；自动配置会根据安装的 Mod 量与电脑剩余内存动态调整；自定义使用下方滑块的值。";
const MEMORY_SLIDER_HELP =
  "拖动滑块设置游戏最大内存；自动配置或跟随全局时由对应规则决定。";
const SERVER_LOGIN_HELP =
  "选择连接服务器时允许的登录方式；第三方登录需要填写 Yggdrasil 认证服务器。";
const AUTH_SERVER_HELP = "填写第三方皮肤站的 Yggdrasil 认证地址。";
const SERVER_REGISTER_HELP =
  "注册账号的网页地址，例如 https://example.com/auth/register。";
const AUTO_JOIN_HELP =
  "在打开 Minecraft 后自动进入某服务器；用英文冒号分隔地址与端口，例如 233.233.233.233:12345。";
const JVM_HELP =
  "启动 Minecraft 时使用的额外 JVM 参数，除非有确定把握，否则请不要修改。";
const GAME_ARGS_HELP =
  "内容会直接拼合在启动参数末尾，例如输入 --demo 以试玩模式启动。";
const GC_HELP =
  "自动添加 GC 参数；只有选择“不指定”时，才能在 Java 虚拟机参数中手动设置。";
const PRE_LAUNCH_HELP = "在游戏启动前执行命令，语法与 Windows 命令提示符一致。";
const MOD_UPDATE_HELP = "禁止为此版本更新 Mod，防止整合包玩家误操作。";
const FILE_CHECK_HELP =
  "完全不更改 assets，也不校验 libraries、第三方登录库与版本主 jar 是否被修改；没有修改相关文件时请勿勾选。";
const WRAPPER_HELP =
  "是否使用 Java Launch Wrapper 修复 Java 18- 在中文路径下可能无法正常启动的问题。";
const LWJGL_HELP =
  "是否使用 LWJGL Unsafe Agent 修复 LWJGL 3.4.1 的一个性能问题。";

export function SettingsPanel({ root, versionId, javas }: SettingsPanelProps) {
  const { settings: appSettings } = useSettings();
  const [settings, setSettings] = useState<VersionSettings | null>(null);
  const latest = useRef<VersionSettings | null>(null);
  const revision = useRef(0);
  const [global, setGlobal] = useState<GameSettings | null>(null);
  const [versionDetail, setVersionDetail] = useState<VersionDetail | null>(
    null
  );
  const [javaSelection, setJavaSelection] = useState<JavaSelection | null>(
    null
  );
  const [javaResolveError, setJavaResolveError] = useState("");
  const [javaResolving, setJavaResolving] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [saveError, setSaveError] = useState("");
  const [saving, setSaving] = useState(false);
  const [nonce, setNonce] = useState(0);
  const [runtime, setRuntime] = useState<GameRuntimeInfo | null>(null);
  const [runtimeError, setRuntimeError] = useState("");
  const [memoryPlan, setMemoryPlan] = useState<MemoryPlanInfo | null>(null);
  const [memoryPlanError, setMemoryPlanError] = useState("");

  useEffect(() => {
    let alive = true;
    setRuntimeError("");
    void loadGameRuntimeInfo()
      .then((info) => {
        if (alive) setRuntime(info);
      })
      .catch((err: unknown) => {
        if (alive) {
          setRuntime(null);
          setRuntimeError(describeError(err));
        }
      });
    return () => {
      alive = false;
    };
  }, [nonce]);

  useEffect(() => {
    let alive = true;
    setMemoryPlanError("");
    void resolveMemoryPlan(root, versionId)
      .then((plan) => {
        if (alive) setMemoryPlan(plan);
      })
      .catch((err: unknown) => {
        if (alive) {
          setMemoryPlan(null);
          setMemoryPlanError(`无法读取内存规划：${describeError(err)}`);
        }
      });
    return () => {
      alive = false;
    };
  }, [root, versionId, nonce]);

  useEffect(() => {
    let alive = true;
    latest.current = null;
    revision.current += 1;
    setSettings(null);
    setVersionDetail(null);
    setLoadError("");
    setSaveError("");
    void Promise.all([
      fetchVersionSettings(root, versionId),
      loadGameSettings(),
      fetchVersionDetail(root, versionId).catch(() => null),
    ])
      .then(([loaded, defaults, detail]) => {
        if (!alive) return;
        latest.current = loaded;
        setSettings(loaded);
        setGlobal(defaults);
        setVersionDetail(detail);
      })
      .catch((err: unknown) => {
        if (alive) setLoadError(describeError(err));
      });
    return () => {
      alive = false;
      revision.current += 1;
    };
  }, [root, versionId, nonce]);

  useEffect(() => {
    let alive = true;
    let timer: number | undefined;
    if (!versionDetail || !settings || !global) {
      setJavaSelection(null);
      setJavaResolveError("");
      setJavaResolving(false);
      return () => {
        alive = false;
      };
    }

    const javaPath = settings.javaPath ?? global.javaPath ?? "";
    setJavaSelection(null);
    setJavaResolveError("");
    setJavaResolving(true);
    timer = window.setTimeout(() => {
      void resolveJava({
        mcVersion:
          versionDetail.minecraftVersion || versionDetail.id || versionId,
        loader:
          versionDetail.loader && versionDetail.loader !== "unknown"
            ? versionDetail.loader
            : null,
        loaderVersion: versionDetail.loaderVersion,
        releaseTime: versionDetail.releaseTime,
        javaPath: javaPath.trim() ? javaPath : null,
      })
        .then((selection) => {
          if (alive) setJavaSelection(selection);
        })
        .catch((error: unknown) => {
          if (alive) setJavaResolveError(describeError(error));
        })
        .finally(() => {
          if (alive) setJavaResolving(false);
        });
    }, 300);

    return () => {
      alive = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [versionDetail, settings?.javaPath, global?.javaPath, versionId]);

  // 不把待保存数据放在组件定时器中，切页后仍由实例级队列完成写入。
  const persist = useCallback(
    (next: VersionSettings) => {
      latest.current = next;
      setSettings(next);
      setSaveError("");
      setSaving(true);
      const current = ++revision.current;
      void saveVersionSettings(root, versionId, next)
        .catch((err: unknown) => {
          if (current === revision.current) setSaveError(describeError(err));
        })
        .finally(() => {
          if (current === revision.current) setSaving(false);
        });
    },
    [root, versionId]
  );

  const patch = useCallback(
    (part: Partial<VersionSettings>) => {
      if (latest.current) persist({ ...latest.current, ...part });
    },
    [persist]
  );

  if (loadError) {
    return (
      <div className="vst-panel">
        <p className="vst-alert" role="alert">
          {loadError}
        </p>
        <Group title="设置">
          <Button
            variant="secondary"
            onClick={() => setNonce((value) => value + 1)}
          >
            重试
          </Button>
        </Group>
      </div>
    );
  }

  if (!settings || !global) {
    return (
      <div className="vst-panel">
        <LoadingCard label="正在读取版本设置..." />
      </div>
    );
  }

  const globalMemoryMb = global.memoryMb;
  const effectiveMemoryMode =
    settings.memoryMode === "global" ? global.memoryMode : settings.memoryMode;
  const suggestedMemoryMb = memoryPlan?.suggestedMemoryMb ?? null;
  const allocatedMemoryMb =
    effectiveMemoryMode === "auto"
      ? suggestedMemoryMb
      : settings.memoryMode === "global"
      ? globalMemoryMb
      : settings.memoryMb ?? globalMemoryMb;
  const globalInstanceSettings = normalizeSettings(global);
  const globalServerLogin = globalInstanceSettings.serverLogin ?? "any";
  const effectiveServerLogin = settings.serverLogin ?? globalServerLogin;
  const manualMemoryMax = Math.max(
    MEMORY_MIN_MB,
    Math.min(MEMORY_MAX_MB, runtime?.totalMemoryMb ?? MEMORY_MAX_MB)
  );
  const effectiveJavaPath = settings.javaPath ?? global.javaPath;
  const javaHint = javaResolving
    ? "正在解析实际使用的 Java..."
    : javaResolveError
    ? `无法解析实际 Java：${javaResolveError}`
    : javaSelection
    ? describeJavaSelection(javaSelection)
    : effectiveJavaPath
    ? `将使用 ${effectiveJavaPath}`
    : "正在读取实例版本信息并解析 Java";
  const javaOptions = [
    {
      value: "global",
      label: "跟随全局",
      tooltip: global.javaPath || "使用全局 Java 设置",
    },
    { value: "", label: "自动选择", tooltip: JAVA_HELP },
    ...javas.map((java) => ({
      value: java.path,
      label: `${java.version || "未知版本"} · ${java.vendor}`,
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
  const memoryManagementOptions = [
    {
      value: "global",
      label: `跟随全局（${
        MEMORY_MANAGEMENT_OPTIONS.find(
          (option) => option.value === global.memoryManagement
        )?.label ?? "调优 G1GC"
      }）`,
    },
    ...MEMORY_MANAGEMENT_OPTIONS,
  ];

  return (
    <div className="vst-panel">
      <p className="vst-note">这些设置只对该游戏版本生效，不影响其他版本。</p>
      {saveError ? (
        <p className="vst-alert" role="alert">
          {saveError}
          <Button size="sm" disabled={saving} onClick={() => persist(settings)}>
            重试保存
          </Button>
        </p>
      ) : null}
      {/*
        保存状态常驻一行固定高度：切换「跟随全局 / 单独设置」时只淡入淡出，
        不再因为提示条瞬间插入 / 移除而让整页内容上下抖动。
      */}
      <div className="vst-save-status" role="status" aria-live="polite">
        <span className="vst-saving" data-visible={saving ? "true" : "false"}>
          正在保存...
        </span>
      </div>
      <Group title="启动选项">
        <Row
          label="版本隔离"
          help={VERSION_ISOLATION_HELP}
          hint="开启后使用独立的存档、配置与资源目录"
        >
          <InheritedBoolean
            label="版本隔离"
            value={settings.versionIsolation}
            globalValue={global.versionIsolation}
            onChange={(value) => patch({ versionIsolation: value })}
          />
        </Row>
        <Row label="游戏窗口标题" help={WINDOW_TITLE_HELP}>
          <InheritedText
            label="游戏窗口标题"
            value={settings.windowTitle}
            globalValue={global.windowTitle}
            onChange={(value) => patch({ windowTitle: value })}
            placeholder="默认"
          />
        </Row>
        <Row label="自定义信息" help={CUSTOM_INFO_HELP}>
          <InheritedText
            label="自定义信息"
            value={settings.customInfo}
            globalValue={global.customInfo}
            onChange={(value) => patch({ customInfo: value })}
            placeholder="默认"
          />
        </Row>
        <Row label="Java">
          <Select
            value={settings.javaPath ?? "global"}
            ariaLabel="Java 选择"
            options={javaOptions}
            onChange={(value) =>
              patch({ javaPath: value === "global" ? null : value })
            }
          />
        </Row>
        <p className="vst-row-note" role="status">
          {javaHint}
        </p>
      </Group>
      <Group title="内存分配">
        <Row label="分配方式" help={MEMORY_MODE_HELP}>
          <SegmentedControl
            value={settings.memoryMode}
            onChange={(value) =>
              patch({
                memoryMode: value as MemoryMode,
                memoryMb:
                  value === "custom" && settings.memoryMb === null
                    ? suggestedMemoryMb ?? globalMemoryMb
                    : settings.memoryMb,
              })
            }
            options={MEMORY_MODE_OPTIONS}
          />
        </Row>
        <Collapse open={settings.memoryMode === "custom"} staggered>
          <Row label="最大内存" help={MEMORY_SLIDER_HELP}>
            <Slider
              min={MEMORY_MIN_MB}
              max={manualMemoryMax}
              step={MEMORY_STEP_MB}
              value={settings.memoryMb ?? globalMemoryMb}
              suffix=" MB"
              aria-label="最大内存"
              onChange={(value) => patch({ memoryMb: Math.round(value) })}
            />
          </Row>
        </Collapse>

        <Collapse open={Boolean(runtimeError || memoryPlanError)} staggered>
          <p className="vst-alert" role="alert">
            {[runtimeError, memoryPlanError].filter(Boolean).join("；")}
          </p>
        </Collapse>
        <GameMemory
          totalMemoryMb={runtime?.totalMemoryMb ?? null}
          availableMemoryMb={runtime?.availableMemoryMb ?? null}
          suggestedMemoryMb={suggestedMemoryMb}
          allocatedMemoryMb={allocatedMemoryMb}
        />
      </Group>
      <Group title="服务器">
        <Row label="登录方式" help={SERVER_LOGIN_HELP}>
          <Select
            ariaLabel="服务器登录方式"
            value={settings.serverLogin ?? "global"}
            options={[
              {
                value: "global",
                label: `跟随全局（${
                  SERVER_LOGIN_OPTIONS.find(
                    (option) => option.value === globalServerLogin
                  )?.label
                }）`,
              },
              ...SERVER_LOGIN_OPTIONS,
            ]}
            onChange={(value) =>
              patch({
                serverLogin: value === "global" ? null : (value as ServerLogin),
              })
            }
          />
        </Row>
        <Collapse open={effectiveServerLogin === "thirdParty"} staggered>
          <Row label="认证服务器" help={AUTH_SERVER_HELP}>
            <InheritedText
              label="认证服务器"
              value={settings.authServer}
              globalValue={globalInstanceSettings.authServer ?? ""}
              onChange={(value) => patch({ authServer: value })}
              placeholder="https://…/api/yggdrasil"
            />
          </Row>
          <Row label="注册链接" help={SERVER_REGISTER_HELP}>
            <InheritedText
              label="注册链接"
              value={settings.serverRegistration}
              globalValue={globalInstanceSettings.serverRegistration ?? ""}
              onChange={(value) => patch({ serverRegistration: value })}
              placeholder="https://…/auth/register"
            />
          </Row>
          <SkinAuthSettings
            server={settings.authServer ?? globalInstanceSettings.authServer ?? appSettings.littleskinServer}
            method={settings.skinAuthMethod}
            globalMethod={appSettings.skinAuthMethod}
            root={root} versionId={versionId}
            onChange={(method) => patch({ skinAuthMethod: method })}
          />
        </Collapse>
        <Row label="自动进入服务器" help={AUTO_JOIN_HELP}>
          <InheritedText
            label="自动进入服务器"
            value={settings.serverAddress}
            globalValue={globalInstanceSettings.serverAddress ?? ""}
            onChange={(value) => patch({ serverAddress: value })}
            placeholder="服务器地址或 IP:端口"
          />
        </Row>
        <div className="vst-actions vst-actions--start">
          <Button
            variant="secondary"
            disabled={saving}
            onClick={() => patch(LITTLESKIN_PRESET)}
          >
            设置为 LittleSkin
          </Button>
        </div>
      </Group>
      <Group title="高级选项">
        <Row label="JVM 参数" help={JVM_HELP}>
          <InheritedText
            label="JVM 参数"
            value={settings.jvmArgs}
            globalValue={global.jvmArgs}
            onChange={(value) => patch({ jvmArgs: value })}
            multiline
            placeholder="额外 JVM 参数"
          />
        </Row>
        <Row label="游戏参数" help={GAME_ARGS_HELP}>
          <InheritedText
            label="游戏参数"
            value={settings.gameArgs}
            globalValue={global.gameArgs}
            onChange={(value) => patch({ gameArgs: value })}
            placeholder="例如 --demo"
          />
        </Row>
        <Row label="内存管理" help={GC_HELP}>
          <Select
            ariaLabel="内存管理"
            value={settings.memoryManagement ?? "global"}
            options={memoryManagementOptions}
            onChange={(value) =>
              patch({
                memoryManagement:
                  value === "global"
                    ? null
                    : (value as GameSettings["memoryManagement"]),
              })
            }
          />
        </Row>
        <Row label="启动前执行命令" help={PRE_LAUNCH_HELP}>
          <InheritedText
            label="启动前执行命令"
            value={settings.preLaunchCommand}
            globalValue={global.preLaunchCommand}
            onChange={(value) => patch({ preLaunchCommand: value })}
            placeholder="启动前执行的命令"
          />
        </Row>
        <Row label="禁止更新 Mod" help={MOD_UPDATE_HELP}>
          <InheritedBoolean
            label="禁止更新 Mod"
            value={modUpdatesSetting(settings)}
            globalValue={modUpdatesSetting(global) ?? false}
            onChange={(value) =>
              patch({ disableModUpdates: value })
            }
          />
        </Row>
        <Row
          label="关闭文件校验"
          help={FILE_CHECK_HELP}
          hint="启动时跳过文件完整性校验"
        >
          <InheritedBoolean
            label="关闭文件校验"
            value={settings.disableFileCheck}
            globalValue={global.disableFileCheck === true}
            onChange={(value) => patch({ disableFileCheck: value })}
          />
        </Row>
        <Row label="禁用 Java Launch Wrapper" help={WRAPPER_HELP}>
          <InheritedBoolean
            label="禁用 Java Launch Wrapper"
            value={settings.disableWrapper}
            globalValue={global.disableWrapper === true}
            onChange={(value) => patch({ disableWrapper: value })}
          />
        </Row>
        <Row label="禁用 LWJGL Unsafe Agent" help={LWJGL_HELP}>
          <InheritedBoolean
            label="禁用 LWJGL Unsafe Agent"
            value={settings.disableLwjglAgent}
            globalValue={global.disableLwjglAgent === true}
            onChange={(value) => patch({ disableLwjglAgent: value })}
          />
        </Row>
      </Group>
    </div>
  );
}
