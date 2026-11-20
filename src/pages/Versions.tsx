import { useCallback, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { Button, LoadingCard, Modal, Switch, Tooltip } from "../components/ui";
import { useSettings } from "../lib/settings";
import { startManagedDownload } from "../components/download/store";
import { IconCheck, IconRefresh } from "../components/ui/icons";
import {
  IconAlert,
  IconArrowLeft,
  IconDownloadArrow,
  IconFolder,
} from "../components/version/icons";
import { VersionRootNav } from "../components/version/VersionRootNav";
import { InstanceCard } from "../components/version/InstanceCard";
import { confirmDialog, pickDirectory } from "../components/version/dialog";
import {
  describeInstancesError,
  suggestInstanceLabel,
  type InstanceEntry,
} from "../components/version/instancesApi";
import {
  useInstanceRegistry,
  useInstanceScan,
} from "../components/version/useInstances";
import { rootDisplayName } from "../components/version/instanceVisuals";
import { openVersionSettings, setRoute } from "../lib/nav";
import { failureLabel } from "../lib/failure";
import {
  deleteVersion,
  fetchVersionDetail,
  renameVersion,
} from "../components/version/vsettings/api";
import {
  setLaunchInstance,
  setVersionFocus,
  useLaunchInstance,
} from "../lib/instances";
import "./Versions.css";

interface PackSelection {
  path: string;
  name: string;
  minecraftVersion: string;
  loader: { kind: string; version: string } | null;
}

type RootDialog =
  | { kind: "rename"; dir: string }
  | { kind: "import"; dir: string; pack: PackSelection };

/**
 * 空状态 / 错误状态块。
 *
 * 与资源中心的 `.rs-notice` 是同一视觉语言、各自实现：版本页不应反向依赖资源中心
 * 模块；跨页共用时应将状态块放入 `src/components/ui`。
 */
function StateNotice({
  icon,
  title,
  hint,
  detail,
  actions,
}: {
  icon: ReactNode;
  title: string;
  hint: string;
  detail?: string;
  actions?: ReactNode;
}) {
  return (
    <div className="vs-notice">
      <div className="vs-notice__icon">{icon}</div>
      <div className="vs-notice__title">{title}</div>
      <p className="vs-notice__hint">{hint}</p>
      {detail ? (
        <div className="vs-notice__detail ni-mono">{detail}</div>
      ) : null}
      {actions ? <div className="vs-notice__actions">{actions}</div> : null}
    </div>
  );
}

/**
 * 实例与版本独立页（无顶栏 tab 归属，从首页「选择版本」进入）。
 *
 * 布局：左侧管理多个 `.minecraft` 根目录（新增 / 移除 / 激活），右侧展示该目录
 * 下的版本实例卡片。
 *
 * 激活语义：选中某个根目录 = 把它写进 `game.json` 的 `gameDir`，并让
 * 实例注册表的 `active` 指向它；`gameDir` 是唯一真相。
 * 整张卡片是唯一选择入口：点选后写入启动实例并返回首页。收藏、删除与设置
 * 是卡片右下角的独立图标按钮，删除仍走二次确认。
 * 设置始终携带根目录与实例目录身份，显示名称不参与身份解析。
 */
export default function Versions() {
  const {
    roots,
    labels,
    defaultDir,
    active,
    loading,
    busy,
    error: registryError,
    reload,
    activate,
    addRoot,
    renameRoot,
    removeRoot,
  } = useInstanceRegistry();
  const scan = useInstanceScan(active);
  const { instance: launch } = useLaunchInstance();
  const [actionError, setActionError] = useState("");
  const [actionBusy, setActionBusy] = useState(false);
  const [showHidden, setShowHidden] = useState(false);
  const [dialog, setDialog] = useState<RootDialog | null>(null);
  const [dialogName, setDialogName] = useState("");
  const [dialogError, setDialogError] = useState("");
  const { settings } = useSettings();
  const visibleEntries = scan.entries.filter(
    (entry) => showHidden || entry.category !== "hidden"
  );

  /**
   * 前沿计划「实例取名优化」：仅在开关开启时，为新添加的孤立 `.minecraft`
   * 写入上级目录名。取名失败不影响目录添加本身，保持默认展示名即可。
   */
  const applyNaming = useCallback(
    async (dir: string) => {
      if (!settings.instanceNamingOptimize) return;
      const normalised = dir.replace(/[\\/]+$/, "");
      const preset = defaultDir.replace(/[\\/]+$/, "");
      if (preset && normalised.toLowerCase() === preset.toLowerCase()) return;
      const label = await suggestInstanceLabel(dir).catch(() => null);
      if (!label) return;
      await renameRoot(dir, label).catch(() => undefined);
    },
    [settings.instanceNamingOptimize, defaultDir, renameRoot]
  );

  async function runRootAction(action: () => Promise<void>) {
    setActionBusy(true);
    setActionError("");
    try {
      await action();
    } catch (err) {
      setActionError(describeInstancesError(err));
    } finally {
      setActionBusy(false);
    }
  }

  async function createRoot() {
    await runRootAction(async () => {
      const parent = await pickDirectory("选择新建 .minecraft 文件夹的位置");
      if (!parent) return;
      const dir = await invoke<string>("instance_create", { parent });
      await addRoot(dir);
      await applyNaming(dir);
    });
  }

  async function deleteRoot(dir: string) {
    await runRootAction(async () => {
      if (
        !(await confirmDialog(
          `删除此目录及其中所有实例、存档和文件？\n${dir}`,
          "删除磁盘目录"
        ))
      )
        return;
      await invoke("instance_root_delete", { dir });
      // 后端删除成功后再同步注册表、游戏目录和前端选中身份。
      await removeRoot(dir);
      reload();
    });
  }

  async function pickPack() {
    const dir = active;
    await runRootAction(async () => {
      const pack = await invoke<PackSelection | null>("pack_import_pick");
      if (!pack) return;
      setDialogName(pack.name);
      setDialogError("");
      setDialog({ kind: "import", dir, pack });
    });
  }

  async function submitDialog() {
    if (!dialog || actionBusy || !dialogName.trim()) return;
    const submittedDialog = dialog;
    const submittedName = dialogName;
    setActionBusy(true);
    setDialogError("");
    // 命名弹窗不能挡住 FIFO 等待项的查看与取消；错误时恢复原始输入。
    if (submittedDialog.kind === "import") setDialog(null);
    try {
      if (submittedDialog.kind === "rename") {
        await renameRoot(submittedDialog.dir, submittedName.trim());
        setDialog(null);
      } else {
        const { pack, dir } = submittedDialog;
        const taskId = await startManagedDownload(
          {
            dir,
            versionId: pack.minecraftVersion,
            instanceName: submittedName,
            loader: pack.loader?.kind || "",
            loaderVersion: pack.loader?.version || null,
            source: settings.fileDownloadSource,
            kind: "modpack",
          },
          () =>
            invoke<string | null>("modpack_import", {
              path: pack.path,
              dir,
              instanceName: submittedName,
            })
        );
        if (taskId === null) return;
      }
    } catch (err) {
      setDialogName(submittedName);
      setDialog(submittedDialog);
      setDialogError(describeInstancesError(err));
    } finally {
      setActionBusy(false);
    }
  }

  async function handleFavorite(entry: InstanceEntry) {
    setActionBusy(true);
    setActionError("");
    try {
      const detail = await fetchVersionDetail(scan.scannedDir, entry.id);
      await renameVersion(scan.scannedDir, entry.id, {
        name: detail.name,
        description: detail.description,
        category: detail.category,
        favorite: !entry.favorite,
      });
      scan.reload();
    } catch (err) {
      setActionError(describeInstancesError(err));
    } finally {
      setActionBusy(false);
    }
  }

  async function handleDelete(entry: InstanceEntry) {
    setActionError("");
    setActionBusy(true);
    try {
      const confirmed = await confirmDialog(
        `删除实例「${entry.displayName}」及其目录内的全部文件？\n${entry.path}`,
        "删除实例"
      );
      if (!confirmed) return;
      await deleteVersion(scan.scannedDir, entry.id);
      if (launch?.root === scan.scannedDir && launch.versionId === entry.id)
        setLaunchInstance(null);
      setVersionFocus(null);
      scan.reload();
    } catch (err) {
      setActionError(describeInstancesError(err));
    } finally {
      setActionBusy(false);
    }
  }

  const handleAdd = useCallback(async () => {
    setActionError("");
    try {
      const dir = await pickDirectory("选择 .minecraft 实例目录");
      if (!dir) return;
      await addRoot(dir);
      await applyNaming(dir);
    } catch (err) {
      setActionError(describeInstancesError(err));
    }
  }, [addRoot, applyNaming]);

  const handleRemove = useCallback(
    async (dir: string) => {
      setActionError("");
      try {
        const ok = await confirmDialog(
          `从列表中移除「${rootDisplayName(
            dir,
            labels,
            defaultDir
          )}」？之后可通过「新增」重新加入。`,
          "移除实例目录"
        );
        if (!ok) return;
        await removeRoot(dir);
      } catch (err) {
        setActionError(describeInstancesError(err));
      }
    },
    [removeRoot, labels, defaultDir]
  );

  const handleSelectInstance = useCallback(
    async (root: string, entry: InstanceEntry) => {
      if (!root || !entry.hasJson) return;
      setActionError("");
      try {
        // 实例所在目录必须是激活目录（= game.json 的 gameDir），否则先同步激活。
        if (active !== root) await activate(root);
        setLaunchInstance({
          root,
          versionId: entry.id,
          displayName: entry.displayName,
          loader: entry.loader,
          icon: entry.icon,
        });
        // 选完即回首页，启动按钮直接消费这次选择。
        setRoute("home");
      } catch (err) {
        setActionError(describeInstancesError(err));
      }
    },
    [active, activate, setLaunchInstance]
  );

  const handleCardSelect = useCallback(
    (entry: InstanceEntry) => void handleSelectInstance(scan.scannedDir, entry),
    [handleSelectInstance, scan.scannedDir]
  );

  /** 写入版本焦点并跳转版本设置独立页。 */
  const handleCardSettings = useCallback(
    (entry: InstanceEntry) => {
      openVersionSettings({ root: scan.scannedDir, versionId: entry.id });
    },
    [scan.scannedDir]
  );

  const retryButton = (
    <Button variant="primary" iconLeft={<IconRefresh />} onClick={reload}>
      重新读取
    </Button>
  );
  const rescanButton = (
    <Button variant="primary" iconLeft={<IconRefresh />} onClick={scan.reload}>
      重新扫描
    </Button>
  );

  return (
    <div className={`vs-root${loading ? " vs-root--loading" : ""}`}>
      <VersionRootNav
        roots={roots}
        labels={labels}
        defaultDir={defaultDir}
        active={active}
        busy={busy || actionBusy}
        onSelect={(dir) =>
          void activate(dir).catch((err: unknown) =>
            setActionError(describeInstancesError(err))
          )
        }
        onAdd={() => void handleAdd()}
        onCreate={() => void createRoot()}
        onImport={() => void pickPack()}
        onDelete={(dir) => void deleteRoot(dir)}
        onOpen={(dir) => void runRootAction(() => openPath(dir))}
        onRefresh={(dir) =>
          void runRootAction(async () => {
            if (dir !== active) await activate(dir);
            scan.reload();
          })
        }
        onRename={(dir) => {
          setDialog({ kind: "rename", dir });
          setDialogName(rootDisplayName(dir, labels, defaultDir));
          setDialogError("");
        }}
        onRemove={(dir) => void handleRemove(dir)}
      />

      <div className="vs-main">
        <header className="vs-head">
          <div className="vs-head__titles">
            <Button
              className="vs-head__back"
              variant="ghost"
              size="sm"
              iconLeft={<IconArrowLeft />}
              onClick={() => setRoute("home")}
            >
              返回
            </Button>
            <div className="ni-eyebrow">Instances</div>
            <h1 className="vs-head__title">实例与版本</h1>
            <div className="vs-head__meta">
              <IconFolder aria-hidden />
              <span className="vs-head__meta-label">当前实例目录</span>
              {active ? (
                <Tooltip label={active}>
                  <span className="vs-head__root">
                    {rootDisplayName(active, labels, defaultDir)}
                  </span>
                </Tooltip>
              ) : (
                <span className="vs-head__root vs-head__root--empty">
                  未选择
                </span>
              )}
            </div>
            {launch ? (
              <Tooltip label={`${launch.versionId} · ${launch.root}`}>
                <div className="vs-head__launch">
                  <IconCheck aria-hidden />
                  <span>启动实例</span>
                  <span className="vs-head__launch-id">
                    {launch.displayName || launch.versionId}
                  </span>
                </div>
              </Tooltip>
            ) : null}
          </div>

          <div className="vs-head__actions">
            <Button
              variant="ghost"
              iconLeft={<IconDownloadArrow />}
              onClick={() => setRoute("resources")}
            >
              去下载
            </Button>
            <Button
              variant="secondary"
              iconLeft={<IconRefresh />}
              loading={scan.state === "loading"}
              disabled={!active || busy}
              onClick={scan.reload}
            >
              重新扫描
            </Button>
          </div>
        </header>

        <Switch
          checked={showHidden}
          onChange={setShowHidden}
          label="显示隐藏实例"
        />
        {actionError || (roots.length > 0 && registryError) ? (
          <p className="vs-alert" role="alert">
            <IconAlert aria-hidden />
            <span>{actionError || registryError}</span>
          </p>
        ) : null}

        {loading ? (
          <LoadingCard label="正在读取实例目录..." />
        ) : roots.length === 0 ? (
          registryError ? (
            <LoadingCard
              state="error"
              label={failureLabel(registryError)}
              action={retryButton}
            />
          ) : (
            <StateNotice
              icon={<IconFolder />}
              title="尚未注册 .minecraft 目录"
              hint="点击左侧「新增」选择实例目录，该目录下的版本会显示在这里。"
              actions={retryButton}
            />
          )
        ) : scan.state === "loading" ? (
          <LoadingCard label="正在扫描版本实例..." />
        ) : scan.state === "error" ? (
          <LoadingCard
            state="error"
            label={failureLabel(scan.error)}
            action={rescanButton}
          />
        ) : scan.state === "empty" ? (
          <StateNotice
            icon={<IconFolder />}
            title="该实例目录下没有可识别的版本"
            hint="实例目录下需要有 versions 文件夹；把版本放进该文件夹后点击「重新扫描」即可看到。"
            detail={scan.scannedDir}
            actions={rescanButton}
          />
        ) : (
          <div className="vs-grid">
            {!visibleEntries.length ? (
              <p className="vs-nav__empty">
                当前实例均已隐藏，打开「显示隐藏实例」后可恢复分类。
              </p>
            ) : null}
            {visibleEntries.map((entry, index) => (
              <InstanceCard
                key={entry.id}
                entry={entry}
                index={index}
                selected={
                  launch !== null &&
                  launch.root === scan.scannedDir &&
                  launch.versionId === entry.id
                }
                busy={busy || actionBusy}
                onSelect={handleCardSelect}
                onSettings={handleCardSettings}
                onFavorite={(entry) => void handleFavorite(entry)}
                onDelete={(entry) => void handleDelete(entry)}
              />
            ))}
          </div>
        )}
      </div>
      <Modal
        open={dialog !== null}
        title={dialog?.kind === "rename" ? "重命名目录" : "导入整合包"}
        description={dialog?.dir}
        onClose={() => {
          if (!actionBusy) setDialog(null);
        }}
        closeOnBackdrop={!actionBusy}
        footer={
          <>
            <Button
              variant="ghost"
              disabled={actionBusy}
              onClick={() => setDialog(null)}
            >
              取消
            </Button>
            <Button
              variant="primary"
              loading={actionBusy}
              disabled={!dialogName.trim()}
              onClick={() => void submitDialog()}
            >
              {dialog?.kind === "rename" ? "保存名称" : "开始导入"}
            </Button>
          </>
        }
      >
        <label className="vs-dialog-field">
          <span>{dialog?.kind === "rename" ? "目录显示名称" : "实例名称"}</span>
          <input
            className="vs-dialog-input"
            autoFocus
            value={dialogName}
            disabled={actionBusy}
            onChange={(event) => setDialogName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void submitDialog();
              }
            }}
          />
        </label>
        {dialogError ? (
          <p className="vs-alert" role="alert">
            {dialogError}
          </p>
        ) : null}
      </Modal>
    </div>
  );
}
