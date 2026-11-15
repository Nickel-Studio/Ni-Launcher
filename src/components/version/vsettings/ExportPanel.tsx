import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { setRoute } from "../../../lib/nav";
import { useSettings } from "../../../lib/settings";
import { PRODUCT_NAME } from "../../../lib/brand";
import { useDownloadCenter } from "../../download/store";
import { formatBytes } from "../../resources/format";
import { IconBundle } from "../../resources/icons";
import {
  Button,
  Collapse,
  IconCheck,
  IconChevronDown,
  IconMinus,
  LoadingCard,
  Switch,
} from "../../ui";
import { InlineNotice } from "../../ui/InlineNotice";
import { describeError } from "./api";
import {
  BUNDLE_RESOURCES_NOTICE,
  DEFAULT_PACK_VERSION,
  EXPORT_FORMATS,
  EXPORT_GROUPS,
  defaultExportSelection,
  exportCheckState,
  exportDisplayGroup,
  inspectPackExport,
  loadExportConfig,
  matchExportSelection,
  saveExportConfig,
  serializeExportSelection,
  startPackExport,
  toggleExportSelection,
  type ExportConfig,
  type ExportFile,
  type ExportFormat,
  type ExportPreview,
  type ExportSelection,
} from "./export";
import "./ExportPanel.css";

interface ExportPanelProps {
  root: string;
  versionId: string;
}

interface FileGroupProps {
  label: string;
  files: ExportFile[];
  selected: ExportSelection;
  disabled: boolean;
  onToggle: (paths: string[], checked: boolean) => void;
}

const GROUP_PAGE_SIZE = 48;

function FileGroup({
  label,
  files,
  selected,
  disabled,
  onToggle,
}: FileGroupProps) {
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState(0);
  const listId = useId();
  const states = files.map((file) => exportCheckState(file, selected));
  const checkedCount = states.filter((state) => state !== "unchecked").length;
  const checked = states.every((state) => state === "checked");
  const mixed = checkedCount > 0 && !checked;
  const expandable = files.length > 1;
  const lastPage = Math.max(0, Math.ceil(files.length / GROUP_PAGE_SIZE) - 1);
  const currentPage = Math.min(page, lastPage);
  // Collapsed groups never create their item elements; expanded groups are bounded.
  const visible =
    open && expandable
      ? files.slice(
          currentPage * GROUP_PAGE_SIZE,
          (currentPage + 1) * GROUP_PAGE_SIZE
        )
      : [];
  return (
    <div className="vst-export__group">
      <div className="vst-export__group-head">
        <button
          type="button"
          className="vst-export__check"
          role="checkbox"
          aria-label={`导出${label}`}
          aria-checked={mixed ? "mixed" : checked}
          disabled={disabled}
          onClick={() =>
            onToggle(
              files.map((file) => file.path),
              !checked
            )
          }
        >
          {checked ? <IconCheck /> : mixed ? <IconMinus /> : null}
        </button>
        {expandable ? (
          <button
            type="button"
            className="vst-export__group-toggle"
            aria-expanded={open}
            aria-controls={listId}
            onClick={() => setOpen((value) => !value)}
          >
            <span>
              {label}
              <span className="vst-export__count">
                {checkedCount} / {files.length} 项
              </span>
            </span>
            <IconChevronDown
              className={
                open ? "vst-export__chevron is-open" : "vst-export__chevron"
              }
            />
          </button>
        ) : (
          <div className="vst-export__group-summary">
            <span>
              {label}
              <span className="vst-export__inline-name">
                {files[0].displayName}
              </span>
            </span>
            <span className="vst-export__size">
              {mixed
                ? "旧配置部分选择"
                : files[0].kind === "directory"
                ? "整体选择"
                : formatBytes(files[0].sizeBytes ?? 0)}
            </span>
          </div>
        )}
      </div>
      {open && expandable ? (
        <div id={listId}>
          <ul className="vst-export__files">
            {visible.map((file) => {
              const state = exportCheckState(file, selected);
              return (
                <li key={file.path}>
                  <button
                    type="button"
                    className="vst-export__file"
                    role="checkbox"
                    aria-label={`导出${file.displayName}`}
                    aria-checked={
                      state === "mixed" ? "mixed" : state === "checked"
                    }
                    disabled={disabled}
                    onClick={() => onToggle([file.path], state !== "checked")}
                  >
                    <span className="vst-export__item-check" aria-hidden="true">
                      {state === "checked" ? (
                        <IconCheck />
                      ) : state === "mixed" ? (
                        <IconMinus />
                      ) : null}
                    </span>
                    <span className="vst-export__path">{file.displayName}</span>
                    <span className="vst-export__size">
                      {state === "mixed"
                        ? "旧配置部分选择"
                        : file.kind === "directory"
                        ? "整个包 / 目录"
                        : formatBytes(file.sizeBytes ?? 0)}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
          {lastPage > 0 ? (
            <div className="vst-export__pagination">
              <Button
                size="sm"
                disabled={currentPage === 0}
                onClick={() => setPage(currentPage - 1)}
              >
                上一页
              </Button>
              <span>
                {currentPage + 1} / {lastPage + 1}
              </span>
              <Button
                size="sm"
                disabled={currentPage === lastPage}
                onClick={() => setPage(currentPage + 1)}
              >
                下一页
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export function ExportPanel({ root, versionId }: ExportPanelProps) {
  const { settings } = useSettings();
  const center = useDownloadCenter();
  const formatName = useId();
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [name, setName] = useState("");
  const [version, setVersion] = useState(DEFAULT_PACK_VERSION);
  const [format, setFormat] = useState<ExportFormat>("mrpack");
  const [includeLauncher, setIncludeLauncher] = useState(false);
  const [includePersonalization, setIncludePersonalization] = useState(false);
  const [selected, setSelected] = useState<ExportSelection>(() => new Map());
  const [bundleResources, setBundleResources] = useState(false);
  const [modrinthOnly, setModrinthOnly] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [exportTaskId, setExportTaskId] = useState<string | null>(null);
  const mounted = useRef(true);
  const request = useRef(0);

  const load = useCallback(async () => {
    const current = ++request.current;
    setLoading(true);
    setError("");
    try {
      const result = await inspectPackExport(root, versionId);
      if (!mounted.current || current !== request.current) return;
      setPreview(result);
      setName(result.name || versionId);
      setSelected(defaultExportSelection(result));
    } catch (err) {
      if (mounted.current && current === request.current)
        setError(describeError(err));
    } finally {
      if (mounted.current && current === request.current) setLoading(false);
    }
  }, [root, versionId]);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => {
      mounted.current = false;
      request.current += 1;
    };
  }, [load]);

  const grouped = useMemo(() => {
    const groups = EXPORT_GROUPS.map((group) => ({
      ...group,
      files: [] as ExportFile[],
    }));
    const byId = new Map(groups.map((group) => [group.id, group]));
    for (const file of preview?.files ?? [])
      byId.get(exportDisplayGroup(file.group))?.files.push(file);
    return groups.filter((group) => group.files.length > 0);
  }, [preview]);
  const selectedFiles = useMemo(
    () => preview?.files.filter((file) => selected.has(file.path)) ?? [],
    [preview, selected]
  );
  const unknownSize = selectedFiles.some(
    (file) =>
      file.sizeBytes === null || exportCheckState(file, selected) === "mixed"
  );
  const exportTask = center.tasks.find(
    (task) => task.task?.taskId === exportTaskId
  );
  const exporting = exportTask?.status === "running";
  const disabled = busy !== "" || loading || exporting;
  const portable = format === "portable";
  const launcherBlocked =
    portable && includeLauncher && !preview?.launcher.available;
  const settingsBlocked = portable && Boolean(preview?.instanceSettingsError);
  const config = (): ExportConfig => ({
    name,
    version,
    files: serializeExportSelection(selected),
    bundleResources,
    modrinthOnly,
    format,
    includeLauncher: portable && includeLauncher,
    includePersonalization: portable && includePersonalization,
  });

  const toggle = (paths: string[], checked: boolean) =>
    setSelected((previous) => toggleExportSelection(previous, paths, checked));

  const run = async (action: "load" | "save" | "export" | "guide") => {
    if (disabled) return;
    setBusy(action);
    setError("");
    setNotice("");
    try {
      if (action === "load") {
        const loaded = await loadExportConfig();
        if (!loaded || !mounted.current || !preview) return;
        const matched = matchExportSelection(preview, loaded.files);
        setName(loaded.name);
        setVersion(loaded.version);
        setFormat(loaded.format);
        setIncludeLauncher(loaded.includeLauncher);
        setIncludePersonalization(loaded.includePersonalization);
        setBundleResources(loaded.bundleResources);
        setModrinthOnly(loaded.modrinthOnly);
        setSelected(matched.selection);
        setNotice(
          [
            "已读取导出配置",
            matched.missing
              ? `当前实例缺少 ${matched.missing} 个选择项目，请重新检查。`
              : "",
            matched.legacy
              ? "旧配置的具体文件选择已保留；部分选择显示为半选，勾选整个目录才会扩大范围。"
              : "",
          ]
            .filter(Boolean)
            .join("；")
        );
      } else if (action === "save") {
        const path = await saveExportConfig(config());
        if (mounted.current && path !== null) setNotice(`配置已保存：${path}`);
      } else if (action === "export") {
        if (launcherBlocked)
          throw new Error(
            preview?.launcher.detail || `请使用 ${PRODUCT_NAME} 发行版重新导出`
          );
        if (settingsBlocked)
          throw new Error(
            preview?.instanceSettingsError || "实例设置损坏，无法安全写入便携包"
          );
        const taskId = await startPackExport(
          { dir: root, versionId, ...config() },
          settings.fileDownloadSource
        );
        if (mounted.current && taskId !== null) setExportTaskId(taskId);
      } else {
        await openUrl(
          "https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack"
        );
      }
    } catch (err) {
      if (mounted.current) setError(describeError(err));
    } finally {
      if (mounted.current) setBusy("");
    }
  };

  if (loading && !preview) return <LoadingCard label="正在读取导出内容..." />;
  if (!preview)
    return (
      <div className="vst-panel">
        <p className="vst-alert" role="alert">
          {error}
        </p>
        <Button onClick={() => void load()}>重新读取</Button>
      </div>
    );

  return (
    <div className="vst-export">
      {error ? (
        <p className="vst-alert" role="alert">
          {error}
        </p>
      ) : null}
      {notice ? (
        <p className="vst-export__notice" role="status">
          {notice}
        </p>
      ) : null}
      <section
        className="vst-export__card vst-export__identity"
        aria-label="整合包信息"
      >
        <label>
          整合包名称
          <input
            className="vst-input"
            value={name}
            disabled={disabled}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          整合包版本
          <input
            className="vst-input"
            value={version}
            disabled={disabled}
            onChange={(event) => setVersion(event.target.value)}
          />
        </label>
      </section>
      <section
        className="vst-export__card vst-export__format"
        aria-label="导出格式"
      >
        <h3>导出格式</h3>
        <div
          className="vst-export__formats"
          role="radiogroup"
          aria-label="导出格式"
        >
          {EXPORT_FORMATS.map((option) => (
            <label className="vst-export__format-option" key={option.value}>
              <input
                type="radio"
                name={formatName}
                value={option.value}
                checked={format === option.value}
                disabled={disabled}
                onChange={() => setFormat(option.value)}
              />
              <span>
                {option.label}
                <small>{option.extension}</small>
              </span>
            </label>
          ))}
        </div>
        <p className="vst-export__hint">
          {portable
            ? `完整解压后运行 ${PRODUCT_NAME}，首次运行会导入整合包、应用实例设置并下载缺少的游戏文件。`
            : "标准整合包不含实例设置；窗口、内存、服务器与启动参数需在导入后重新设置。"}
        </p>
        <Collapse open={portable} staggered>
          <div className="vst-export__portable">
            <Switch
              label={`附带 ${PRODUCT_NAME} 启动器`}
              description={preview.launcher.detail}
              checked={includeLauncher}
              disabled={disabled}
              onChange={setIncludeLauncher}
            />
            <Switch
              label="附带个性化设置"
              description="使用当前明暗模式、颜色主题与背景图片"
              checked={includePersonalization}
              disabled={disabled}
              onChange={setIncludePersonalization}
            />
            <p className="vst-export__hint">
              迁移内容：
              {preview.instanceSettings.length > 0
                ? preview.instanceSettings.join("、")
                : "未发现可迁移的实例设置"}
              。Java 路径、账号与缓存路径不随包迁移。
            </p>
            {preview.instanceSettingsOmitted.length > 0 ? (
              <p className="vst-export__hint">
                未迁移：{preview.instanceSettingsOmitted.join("、")}
                （包含本机路径或凭据）。
              </p>
            ) : null}
            {preview.instanceSettingsError ? (
              <p className="vst-alert" role="alert">
                {preview.instanceSettingsError}
              </p>
            ) : null}
            {!includeLauncher ? (
              <p className="vst-export__hint">
                将本平台 {PRODUCT_NAME} 发行版放到解压目录再运行；macOS 放置完整
                .app。
              </p>
            ) : null}
          </div>
        </Collapse>
      </section>
      <section className="vst-export__card" aria-label="导出内容列表">
        <h3>导出内容</h3>
        <p className="vst-export__base">
          <IconCheck />
          游戏本体
          <span>
            Minecraft {preview.minecraftVersion}
            {preview.loader
              ? ` · ${preview.loader.kind} ${preview.loader.version}`
              : ""}
          </span>
        </p>
        <p className="vst-export__hint vst-export__selection-hint">
          包与存档按整体选择，配置、地图和截图按目录选择；目录大小及完整文件校验在导出时计算。个人数据默认不选。
        </p>
        {grouped.map((group) => (
          <FileGroup
            key={group.id}
            label={group.label}
            files={group.files}
            selected={selected}
            disabled={disabled}
            onToggle={toggle}
          />
        ))}
        {preview.files.length === 0 ? (
          <p className="vst-export__notice">
            当前实例没有可附加的内容，将导出游戏与加载器依赖信息。
          </p>
        ) : null}
      </section>
      <section
        className="vst-export__card vst-export__advanced"
        aria-label="导出高级选项"
      >
        <h3>高级选项</h3>
        <Switch
          label="导出资源文件"
          description="将所选 Mod、资源包和光影文件写入整合包；游戏本体与 Java 在导入时补全"
          checked={bundleResources}
          disabled={disabled}
          onChange={setBundleResources}
        />
        {bundleResources ? (
          <InlineNotice
            tone="warning"
            title="注意"
            role="status"
            className="vst-export__resource-warning"
          >
            {BUNDLE_RESOURCES_NOTICE}
          </InlineNotice>
        ) : null}
        <Switch
          label="仅从 Modrinth 下载资源文件"
          checked={modrinthOnly}
          disabled={disabled}
          onChange={setModrinthOnly}
        />
        <div className="vst-export__actions">
          <Button
            size="sm"
            disabled={disabled}
            loading={busy === "load"}
            onClick={() => void run("load")}
          >
            读取配置
          </Button>
          <Button
            size="sm"
            disabled={disabled || !name.trim() || !version.trim()}
            loading={busy === "save"}
            onClick={() => void run("save")}
          >
            保存配置
          </Button>
          <Button
            size="sm"
            disabled={disabled}
            onClick={() => void run("guide")}
          >
            Modrinth 格式指南
          </Button>
        </div>
      </section>
      <Collapse open={exportTask !== undefined} staggered>
        {exportTask ? (
          <div className="vst-export__notice" role="status">
            {exportTask.status === "done"
              ? "整合包已导出"
              : exportTask.status === "cancelled"
              ? "导出已取消"
              : exportTask.status === "error"
              ? exportTask.message || "导出失败，请查看任务详情后重试"
              : exportTask.stage || "正在导出整合包"}
            <Button size="sm" onClick={() => setRoute("downloads")}>
              查看任务
            </Button>
          </div>
        ) : null}
      </Collapse>
      <div className="vst-export__footer">
        <span>
          已选 {selectedFiles.length} 项 ·{" "}
          {unknownSize
            ? "完整大小在导出时计算"
            : formatBytes(
                selectedFiles.reduce(
                  (total, file) => total + (file.sizeBytes ?? 0),
                  0
                )
              )}{" "}
          · {portable ? ".zip" : ".mrpack"}
        </span>
        <Button
          variant="primary"
          iconLeft={<IconBundle />}
          disabled={
            disabled ||
            launcherBlocked ||
            settingsBlocked ||
            !name.trim() ||
            !version.trim()
          }
          loading={busy === "export"}
          onClick={() => void run("export")}
        >
          开始导出
        </Button>
      </div>
    </div>
  );
}
