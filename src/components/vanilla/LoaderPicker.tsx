import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button, Collapse, IconButton } from "../ui";
import { IconCheck, IconChevronDown, IconRefresh } from "../ui/icons";
import { IconAlert } from "../resources/icons";
import { IconDownloadTray } from "../download/icons";
import { describeInstallError } from "../download/parse";
import {
  startDownload,
  startInstallerDownload,
  useDownloadState,
} from "../download/store";
import {
  LOADER_IDS,
  LOADER_LABELS,
  type DownloadSource,
  type LoaderCatalogProgress,
  type LoaderId,
  type LoaderOption,
  type LoaderProvider,
  type RemoteVersion,
} from "../download/types";
import { scanInstances } from "../version/instancesApi";
import {
  blockIconSrc,
  formatReleaseTime,
  typeLabel,
} from "../version/instanceVisuals";
import { versionGroupOf } from "../resources/versionGroups";
import { instanceNameError } from "./instanceName";
import { cachedCatalog, initialCatalog, requestCatalog } from "./catalogCache";
import {
  downloadSelectionError,
  isMainLoaderId,
  mainLoaderConflict,
  optifineOptionsView,
  resolvedDownloadIdentity,
  type LoaderOptionView,
  type LoaderSelection,
} from "./loaderCompatibility";

const ICON_KEYS: Record<LoaderId, string> = {
  forge: "anvil",
  neoforge: "neoforge",
  fabric: "fabric",
  liteloader: "egg",
  optifine: "grass_path",
};

type Choice = LoaderSelection | null;
type Notice = { text: string; error?: boolean } | null;

export function LoaderPicker({
  version,
  source,
  fileSource,
  instanceDir,
  onBack,
}: {
  version: RemoteVersion;
  source: DownloadSource;
  fileSource: DownloadSource;
  instanceDir: string;
  onBack: () => void;
}) {
  const [name, setName] = useState(version.id);
  const [catalog, setCatalog] = useState(
    () => cachedCatalog(version, source) ?? initialCatalog(version)
  );
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [operationId] = useState(() => crypto.randomUUID());
  const [refreshing, setRefreshing] = useState(true);
  const [choice, setChoice] = useState<Choice>(null);
  const [expanded, setExpanded] = useState<LoaderId | null>(null);
  const [notice, setNotice] = useState<Notice>(null);
  const [saving, setSaving] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const savingRef = useRef(false);
  const submitRef = useRef(false);
  const alive = useRef(true);
  const requestSequence = useRef(0);
  const selectionKey = useRef(`${version.id}\u0000${source}`);
  const download = useDownloadState();
  const nameError = instanceNameError(name);
  const running = download.status === "running";
  const group = versionGroupOf(version);
  const icon = choice
    ? ICON_KEYS[choice.id]
    : group === "release"
    ? "grass"
    : group === "snapshot"
    ? "command_block"
    : group === "april"
    ? "gold_block"
    : "cobblestone";
  const inputHint =
    nameError || (!instanceDir ? "请先在「版本」页选择游戏目录。" : "");
  const primaryChoice = choice && isMainLoaderId(choice.id) ? choice : null;
  // 若目录层尚未保留 artifact.rawMetadata，Forge 规则会保守降级为仅另存。
  const optifineView = optifineOptionsView(
    catalog.optifine,
    version.id,
    primaryChoice
  );
  const rawOptifineProvider = catalog.providers.optifine;
  const optifineProvider: LoaderProvider =
    rawOptifineProvider.state === "loading" ||
    rawOptifineProvider.state === "failed" ||
    rawOptifineProvider.state === "hidden"
      ? rawOptifineProvider
      : {
          state: optifineView.state,
          capability: {
            install: optifineView.options.some(
              (option) => option.capability.install
            ),
            saveInstaller: optifineView.options.some(
              (option) => option.capability.saveInstaller
            ),
          },
          error: optifineView.reason,
        };
  const choiceOptions: readonly LoaderOption[] =
    choice?.id === "optifine"
      ? optifineView.options
      : choice
      ? catalog[choice.id]
      : [];
  const choiceInvalid =
    choice !== null &&
    !choiceOptions.some(
      (build) => build.version === choice.version && build.capability.install
    );

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    const sequence = ++requestSequence.current;
    const currentSelection = `${version.id}\u0000${source}`;
    if (selectionKey.current !== currentSelection) {
      selectionKey.current = currentSelection;
      setName(version.id);
      setChoice(null);
      setExpanded(null);
      setNotice(null);
    }
    setCatalog(cachedCatalog(version, source) ?? initialCatalog(version));
    setError("");
    setRefreshing(true);
    requestCatalog(
      version,
      source,
      attempt > 0,
      `${operationId}:${attempt}`,
      (progress: LoaderCatalogProgress) => {
        if (!cancelled && sequence === requestSequence.current)
          setCatalog(progress.catalog);
      }
    )
      .then((result) => {
        if (!cancelled && sequence === requestSequence.current)
          setCatalog(result);
      })
      .catch((reason: unknown) => {
        if (cancelled || sequence !== requestSequence.current) return;
        const message = describeInstallError(reason);
        setError(message);
        const failed = initialCatalog(version);
        for (const id of LOADER_IDS) {
          if (failed.providers[id].state === "loading") {
            failed.providers[id] = {
              state: "failed",
              capability: { install: false, saveInstaller: false },
              error: message,
            };
          }
        }
        setCatalog(failed);
      })
      .finally(() => {
        if (!cancelled && sequence === requestSequence.current)
          setRefreshing(false);
      });
    return () => {
      cancelled = true;
    };
  }, [version, source, attempt, operationId]);

  function retry() {
    if (refreshing) return;
    // 立即作废正在返回的旧请求，再由 effect 发起一次真实刷新。
    requestSequence.current += 1;
    setAttempt((value) => value + 1);
  }

  async function save(loader: LoaderId, option: LoaderOption) {
    if (
      savingRef.current ||
      submitRef.current ||
      running ||
      !option.capability.saveInstaller
    )
      return;
    savingRef.current = true;
    setSaving(`${loader}:${option.version}`);
    setNotice(null);
    try {
      // 返回值仅表示任务已启动；完成、失败和取消由统一下载中心反馈。
      await startInstallerDownload({
        mcVersion: version.id,
        loader,
        loaderVersion: option.version,
        source: fileSource,
      });
    } catch (reason) {
      if (alive.current)
        setNotice({ text: describeInstallError(reason), error: true });
    } finally {
      savingRef.current = false;
      if (alive.current) setSaving("");
    }
  }

  async function changelog(option: LoaderOption) {
    try {
      await openUrl(option.changelogUrl);
    } catch (reason) {
      if (alive.current)
        setNotice({
          text: `打开更新日志失败：${describeInstallError(reason)}`,
          error: true,
        });
    }
  }

  async function beginDownload() {
    if (
      submitRef.current ||
      savingRef.current ||
      running ||
      !instanceDir ||
      nameError ||
      choiceInvalid
    )
      return;
    const selectionError = downloadSelectionError(choice ? [choice] : []);
    if (selectionError) {
      setNotice({ text: selectionError, error: true });
      return;
    }
    submitRef.current = true;
    setSubmitting(true);
    setNotice(null);
    try {
      const entries = await scanInstances(instanceDir);
      if (!alive.current) return;
      if (
        entries.some(
          (entry) =>
            entry.path.split(/[\\/]/).pop()?.toLowerCase() ===
            name.toLowerCase()
        )
      ) {
        setNotice({
          text: `实例名称「${name}」已存在，请换一个名称。`,
          error: true,
        });
        return;
      }
      const identity = resolvedDownloadIdentity(choice);
      await startDownload({
        dir: instanceDir,
        versionId: version.id,
        instanceName: name,
        loader: identity.loader,
        loaderVersion: identity.loaderVersion,
        source: fileSource,
      });
    } catch (reason) {
      if (alive.current)
        setNotice({ text: describeInstallError(reason), error: true });
    } finally {
      submitRef.current = false;
      if (alive.current) setSubmitting(false);
    }
  }

  return (
    <section className="van-picker" aria-label={`安装 Minecraft ${version.id}`}>
      <header className="van-picker__head">
        <div className="van-instance">
          <IconButton label="返回版本列表" onClick={onBack}>
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.75"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="m10 5-7 7 7 7M3 12h18" />
            </svg>
          </IconButton>
          <img
            src={blockIconSrc(icon)}
            width={36}
            height={36}
            alt=""
            draggable={false}
          />
          <div className="van-instance__field">
            <input
              value={name}
              onChange={(event) => {
                setName(event.target.value);
                setNotice(null);
              }}
              aria-label="实例名称"
              aria-invalid={!!nameError}
              aria-describedby={inputHint ? "van-name-hint" : undefined}
              autoComplete="off"
              spellCheck={false}
              disabled={submitting}
            />
          </div>
        </div>
        <div className="van-picker__tools">
          <p className="van-picker__meta">
            Minecraft {version.id} · {typeLabel(version.type)}
            {formatReleaseTime(version.releaseTime)
              ? ` · ${formatReleaseTime(version.releaseTime)}`
              : ""}
          </p>
          <IconButton
            size="sm"
            label="重新查询加载器"
            onClick={retry}
            disabled={submitting || refreshing}
          >
            <IconRefresh />
          </IconButton>
        </div>
        {inputHint ? (
          <p
            id="van-name-hint"
            className="van-picker__hint"
            data-error={!!nameError}
            role="alert"
          >
            {inputHint}
          </p>
        ) : null}
      </header>
      {error ? (
        <div className="mr-page__notice" role="alert">
          <IconAlert />
          <p>加载失败：{error}</p>
        </div>
      ) : null}
      <ul className="van-loaders" aria-label="选择加载器" inert={submitting}>
        {LOADER_IDS.map((id) => {
          const rawProvider =
            id === "optifine" ? optifineProvider : catalog.providers[id];
          const hidden = rawProvider.state === "hidden";
          // 互斥状态不能把本就不适用于当前 MC 的加载器重新显示出来。
          const blockedReason = hidden ? "" : mainLoaderConflict(choice, id);
          const provider: LoaderProvider = blockedReason
            ? {
                state: "incompatible",
                capability: { install: false, saveInstaller: false },
                error: blockedReason,
              }
            : rawProvider;
          const options = hidden
            ? []
            : id === "optifine"
            ? optifineView.options
            : catalog[id];
          return (
            <LoaderCard
              key={id}
              id={id}
              options={options}
              provider={provider}
              blockedReason={blockedReason}
              open={expanded === id}
              picked={choice?.id === id ? choice.version : ""}
              onToggle={() =>
                blockedReason
                  ? undefined
                  : setExpanded((current) => (current === id ? null : id))
              }
              onPick={(build) => {
                if (blockedReason) return;
                if (
                  id === "optifine" &&
                  build &&
                  !optifineView.options.some(
                    (option) =>
                      option.version === build && option.capability.install
                  )
                ) {
                  return;
                }
                setChoice(build ? { id, version: build } : null);
                setExpanded(null);
              }}
              refreshing={refreshing}
              onRetry={retry}
              saving={saving}
              saveDisabled={running || !!saving}
              onSave={(option) => void save(id, option)}
              onChangelog={(option) => void changelog(option)}
            />
          );
        })}
      </ul>
      {primaryChoice && optifineView.state === "manualOnly" ? (
        <p className="van-picker__notice" role="status">
          已选 {LOADER_LABELS[primaryChoice.id]}。OptiFine
          行仅提供另存；共装时请手动放入对应 mods 目录。
        </p>
      ) : null}
      {choiceInvalid ? (
        <div className="mr-page__notice" role="alert">
          <IconAlert />
          <p>所选构建暂不可用，请重试或移除加载器。</p>
          <Button size="sm" onClick={() => setChoice(null)}>
            移除加载器
          </Button>
        </div>
      ) : null}
      {notice ? (
        <p
          className="van-picker__notice"
          data-error={!!notice.error}
          role={notice.error ? "alert" : "status"}
        >
          {notice.text}
        </p>
      ) : null}
      {download.status === "error" && download.task?.instanceName === name ? (
        <p className="van-picker__notice" data-error="true" role="alert">
          {download.message}
        </p>
      ) : null}
      <footer className="van-picker__foot">
        <Button
          className="van-picker__download"
          variant="primary"
          iconLeft={<IconDownloadTray />}
          disabled={
            !instanceDir || !!nameError || running || !!saving || choiceInvalid
          }
          loading={submitting}
          onClick={() => void beginDownload()}
        >
          {running ? "正在下载" : "开始下载"}
        </Button>
      </footer>
    </section>
  );
}

function LoaderCard({
  id,
  options,
  provider,
  blockedReason,
  open,
  picked,
  onToggle,
  onPick,
  refreshing,
  onRetry,
  saving,
  saveDisabled,
  onSave,
  onChangelog,
}: {
  id: LoaderId;
  options: readonly LoaderOptionView[];
  provider: LoaderProvider;
  blockedReason: string;
  open: boolean;
  picked: string;
  onToggle: () => void;
  onPick: (version: string) => void;
  refreshing: boolean;
  onRetry: () => void;
  saving: string;
  saveDisabled: boolean;
  onSave: (option: LoaderOptionView) => void;
  onChangelog: (option: LoaderOptionView) => void;
}) {
  const head = useRef<HTMLButtonElement>(null);
  if (provider.state === "hidden" && options.length === 0) return null;
  const blocked = !!blockedReason;
  const buildCount = options.length;
  const installableCount = options.filter(
    (option) => option.capability.install
  ).length;
  const saveableCount = options.filter(
    (option) => option.capability.saveInstaller
  ).length;
  const canExpand = buildCount > 0 && !blocked;
  const failed = !blocked && provider.state === "failed";
  const isOpen = canExpand && open;
  const featured = options.filter(
    (option) => option.recommended || option.latest
  );
  const label = LOADER_LABELS[id];
  const countLabel = `${buildCount} 个版本`;
  const status = blocked
    ? blockedReason
    : failed
    ? "加载失败"
    : provider.state === "loading"
    ? buildCount > 0
      ? `正在加载 · ${countLabel}`
      : "正在加载"
    : provider.state === "incompatible"
    ? provider.error || "与当前组合不兼容"
    : buildCount === 0
    ? "无可用构建"
    : installableCount > 0
    ? picked || countLabel
    : saveableCount > 0
    ? `仅可另存 · ${countLabel}`
    : `${countLabel} · 暂不可安装`;

  function pick(version: string) {
    onPick(version);
    head.current?.focus({ preventScroll: true });
  }

  function row(option: LoaderOptionView, prefix: string) {
    return (
      <BuildRow
        key={`${prefix}:${option.version}`}
        id={id}
        option={option}
        picked={picked === option.version}
        saving={saving}
        saveDisabled={saveDisabled}
        onPick={() => pick(option.version)}
        onSave={() => onSave(option)}
        onChangelog={() => onChangelog(option)}
      />
    );
  }
  return (
    <li
      className="van-loader"
      data-loader={id}
      data-open={isOpen}
      data-selected={!!picked}
      data-state={blocked ? "incompatible" : provider.state}
      data-blocked={blocked}
    >
      <div className="van-loader__heading">
        <button
          ref={head}
          type="button"
          className="van-loader__head"
          onClick={canExpand ? onToggle : undefined}
          disabled={!canExpand}
          title={blockedReason || undefined}
          aria-expanded={canExpand ? isOpen : undefined}
          aria-controls={canExpand ? `van-builds-${id}` : undefined}
        >
          <img
            className="van-loader__icon"
            src={blockIconSrc(ICON_KEYS[id])}
            width={28}
            height={28}
            alt=""
            draggable={false}
          />
          <span className="van-loader__name">{label}</span>
          <span
            className="van-loader__status"
            title={blockedReason || provider.error || undefined}
          >
            {status}
          </span>
          {picked && canExpand ? (
            <IconCheck className="van-loader__check" />
          ) : null}
          {canExpand ? (
            <IconChevronDown className="van-loader__chevron" />
          ) : null}
        </button>
        {failed ? (
          <Button
            className="van-loader__retry"
            size="sm"
            onClick={onRetry}
            disabled={refreshing}
            title={provider.error}
            aria-label={`重试 ${label}：${provider.error}`}
          >
            重试
          </Button>
        ) : null}
      </div>
      {canExpand ? (
        <Collapse
          className="van-loader__expansion"
          open={isOpen}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation();
              onToggle();
              head.current?.focus();
            }
          }}
        >
          <div
            id={`van-builds-${id}`}
            className="van-loader__body"
            role="region"
            aria-label={`${label} 版本列表`}
          >
            {picked ? (
              <div className="van-loader__intro">
                <Button size="sm" variant="ghost" onClick={() => pick("")}>
                  不添加 {label}
                </Button>
              </div>
            ) : null}
            {featured.length ? (
              <ul
                className="van-builds van-builds--featured"
                aria-label={`${label} 推荐与最新版本`}
              >
                {featured.map((option) => row(option, "featured"))}
              </ul>
            ) : null}
            <div className="van-loader__all">
              全部版本 <span>({buildCount})</span>
            </div>
            <ul
              className="van-builds van-builds--all"
              aria-label={`${label} 全部版本`}
            >
              {options.map((option) => row(option, "all"))}
            </ul>
          </div>
        </Collapse>
      ) : null}
    </li>
  );
}

function BuildRow({
  id,
  option,
  picked,
  saving,
  saveDisabled,
  onPick,
  onSave,
  onChangelog,
}: {
  id: LoaderId;
  option: LoaderOptionView;
  picked: boolean;
  saving: string;
  saveDisabled: boolean;
  onPick: () => void;
  onSave: () => void;
  onChangelog: () => void;
}) {
  const dateValue = option.releaseTime || option.modifiedTime;
  const date = dateValue ? new Date(dateValue) : null;
  const released =
    date && Number.isFinite(date.getTime())
      ? new Intl.DateTimeFormat("zh-CN", {
          year: "numeric",
          month: "2-digit",
          day: "2-digit",
          hour: "2-digit",
          minute: "2-digit",
          hour12: false,
        }).format(date)
      : "";
  const installable = option.capability.install;
  const availability = option.manualReason
    ? option.manualReason
    : installable
    ? ""
    : option.capability.saveInstaller
    ? "仅可另存安装器"
    : "暂不可自动安装";
  const tags = [
    option.recommended && "推荐版",
    option.latest && "最新版",
    option.stable === true && !option.recommended && "稳定版",
    option.stable === false && "预览版",
  ].filter(Boolean);
  const detail = [
    ...tags,
    availability,
    released && `${option.releaseTime ? "发布于" : "更新于"} ${released}`,
  ]
    .filter(Boolean)
    .join(" · ");
  const busy = saving === `${id}:${option.version}`;
  return (
    <li
      className="van-build"
      data-installable={installable}
      data-selected={picked}
      data-compatibility={option.compatibility}
      data-version={option.version}
    >
      <button
        type="button"
        className="van-build__pick"
        aria-pressed={picked}
        onClick={onPick}
        disabled={!installable}
        title={availability || undefined}
      >
        <img
          className="van-loader__icon"
          src={blockIconSrc(ICON_KEYS[id])}
          width={28}
          height={28}
          alt=""
          draggable={false}
        />
        <span className="van-build__text">
          <span className="van-build__version">{option.label}</span>
          {detail ? <span className="van-build__meta">{detail}</span> : null}
        </span>
        {picked ? <IconCheck className="van-build__check" /> : null}
      </button>
      <div className="van-build__actions">
        {option.capability.saveInstaller ? (
          <IconButton
            size="sm"
            label={
              id === "fabric"
                ? `另存为 Fabric 官方安装器（安装时选择 ${option.version}）`
                : id === "optifine"
                ? `另存 OptiFine ${option.label} 原始文件`
                : `另存为 ${LOADER_LABELS[id]} ${option.version} 安装器`
            }
            disabled={saveDisabled}
            aria-busy={busy}
            onClick={onSave}
          >
            {busy ? (
              <span className="ni-btn__spinner" aria-hidden="true" />
            ) : (
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.75"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <path d="M5 3h12l4 4v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z" />
                <path d="M7 3v6h10V3M7 21v-8h10v8M9 17h6" />
              </svg>
            )}
          </IconButton>
        ) : null}
        {option.changelogUrl ? (
          <IconButton
            size="sm"
            label={`${LOADER_LABELS[id]} ${option.version} 更新日志`}
            onClick={onChangelog}
          >
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.75"
              strokeLinecap="round"
              aria-hidden="true"
            >
              <circle cx="12" cy="12" r="9" />
              <path d="M12 11v6M12 7h.01" />
            </svg>
          </IconButton>
        ) : null}
      </div>
    </li>
  );
}
