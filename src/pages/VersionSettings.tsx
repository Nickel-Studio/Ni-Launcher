/**
 * 版本设置独立页，无顶栏 tab 归属。
 *
 * 左侧 `PagerLayout`：概览 / 设置；右上角返回实例页。
 * 页面加载时从 `useVersionFocus()` 拿到目标实例（root + versionId），
 * 详情和设置请求共用该身份；显示名称不能替代 root + versionId。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { openResourceMods, useAppNav } from "../lib/nav";
import { failureLabel } from "../lib/failure";

import {
  Button,
  IconRefresh,
  LoadingCard,
  PagerLayout,
  type PagerItem,
} from "../components/ui";
import { IconLayers, IconSliders } from "../components/account/icons";
import {
  IconBundle,
  IconFolderOpen,
  IconMods,
} from "../components/resources/icons";
import { ModsPanel } from "../components/version/vsettings/ModsPanel";
import { ExportPanel } from "../components/version/vsettings/ExportPanel";
import { ResourceCard } from "../components/resources/ResourceCard";
import {
  ResourceNotice,
  ResourceSkeletonGrid,
} from "../components/resources/ResourceNotice";
import { useResources } from "../components/resources/useResources";
import {
  OverviewPanel,
  SettingsPanel,
  type OverviewData,
} from "../components/version/vsettings/panels";
import {
  setVersionFocus,
  useVersionFocus,
} from "../components/version/vsettings/focus";
import {
  describeError,
  detectJava,
  fetchVersionDetail,
  type JavaInstallation,
} from "../components/version/vsettings/api";
import "./VersionSettings.css";
const SECTIONS: PagerItem[] = [
  { id: "overview", label: "概览", icon: <IconLayers /> },
  { id: "settings", label: "设置", icon: <IconSliders /> },
  { id: "mods", label: "Mod 管理", icon: <IconMods /> },
  { id: "export", label: "导出", icon: <IconBundle /> },
  { id: "installed", label: "已安装资源", icon: <IconFolderOpen /> },
];

/** 概览 / 设置面板需要的最小数据集。 */
export interface VersionPageData {
  root: string;
  versionId: string;
  detail: OverviewData;
  javas: JavaInstallation[];
}

/** 页面加载：焦点 + 详情 + Java 探测并行拉取。 */
function useVersionPage(): {
  data: VersionPageData | null;
  error: string;
  loading: boolean;
  reload: () => void;
  patchDetail: (patch: Partial<OverviewData>) => void;
} {
  const { focus } = useVersionFocus();
  const [detail, setDetail] = useState<OverviewData | null>(null);
  const [javas, setJavas] = useState<JavaInstallation[]>([]);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [nonce, setNonce] = useState(0);

  const reload = useCallback(() => setNonce((n) => n + 1), []);
  /**
   * 概览里的改名 / 分类 / 收藏只回传变化字段：原地合并后立即驱动卡片，
   * 不再走 `reload()` 的整页加载态。真实落盘仍由各操作自己完成。
   */
  const patchDetail = useCallback((patch: Partial<OverviewData>) => {
    setDetail((previous) => (previous ? { ...previous, ...patch } : previous));
  }, []);

  useEffect(() => {
    if (!focus) {
      setDetail(null);
      setError("");
      setLoading(false);
      return;
    }
    let alive = true;
    setLoading(true);
    setError("");
    void Promise.all([
      fetchVersionDetail(focus.root, focus.versionId),
      detectJava().catch(() => [] as JavaInstallation[]),
    ])
      .then(([detail, javas]) => {
        if (!alive) return;
        setDetail({ ...detail, id: focus.versionId });
        setJavas(javas);
      })
      .catch((err: unknown) => {
        if (alive) {
          setDetail(null);
          setError(describeError(err));
        }
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [focus, nonce]);

  const data = useMemo<VersionPageData | null>(() => {
    if (!focus || !detail) return null;
    return {
      root: focus.root,
      versionId: focus.versionId,
      detail: { ...detail, dir: detail.dir || focus.root },
      javas,
    };
  }, [focus, detail, javas]);
  return { data, error, loading, reload, patchDetail };
}

/** 返回实例页并清空焦点，避免下次进入残留旧目标。 */
function useBackToInstances() {
  const { setRoute } = useAppNav();
  return useCallback(() => {
    setVersionFocus(null);
    setRoute("instances");
  }, [setRoute]);
}

export default function VersionSettings() {
  const [active, setActive] = useState<string>(SECTIONS[0].id);

  const { data, error, loading, reload, patchDetail } = useVersionPage();
  const goBack = useBackToInstances();

  return (
    <div className="vst-root">
      <div className="vst-topbar">
        <button type="button" className="vst-back" onClick={goBack}>
          <svg
            viewBox="0 0 24 24"
            width="1em"
            height="1em"
            fill="none"
            stroke="currentColor"
            strokeWidth={1.75}
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden
            focusable="false"
          >
            <path d="M14.5 5.5 8 12l6.5 6.5" />
          </svg>
          <span>返回实例列表</span>
        </button>
      </div>

      <PagerLayout
        className="ni-pager--fill vst-pager"
        items={SECTIONS}
        active={active}
        onSelect={setActive}
        navLabel="版本页分区"
        navTitle={data ? data.detail.name || data.versionId : "版本设置"}
      >
        {loading ? (
          <LoadingCard label="正在读取版本信息..." />
        ) : !data && error ? (
          <LoadingCard
            state="error"
            label={failureLabel(error)}
            action={<Button onClick={reload}>重新读取</Button>}
          />
        ) : !data ? (
          <div className="vst-notice" role="status">
            <h2 className="vst-notice__title">尚未选择实例</h2>
            <p className="vst-notice__hint">
              从实例列表选择一个实例，即可管理它的个性化信息与启动设置。
            </p>
            <Button onClick={goBack}>选择实例</Button>
          </div>
        ) : (
          <>
            {error ? (
              <p className="vst-alert" role="alert">
                {error}
              </p>
            ) : null}
            {active === "overview" ? (
              <OverviewPanel
                data={data.detail}
                root={data.root}
                onChanged={patchDetail}
                key={`${data.root}/${data.versionId}`}
              />
            ) : active === "mods" ? (
              <ModsPanel
                root={data.root}
                versionId={data.versionId}
                displayName={data.detail.name || data.versionId}
                onBrowse={() =>
                  openResourceMods({
                    root: data.root,
                    versionId: data.versionId,
                  })
                }
                onSettings={() => setActive("settings")}
                key={`${data.root}/${data.versionId}`}
              />
            ) : active === "export" ? (
              <ExportPanel
                root={data.root}
                versionId={data.versionId}
                key={`${data.root}/${data.versionId}`}
              />
            ) : active === "installed" ? (
              <InstalledPanel
                root={data.root}
                versionId={data.versionId}
                key={`${data.root}/${data.versionId}`}
              />
            ) : (
              <SettingsPanel
                root={data.root}
                versionId={data.versionId}
                javas={data.javas}
                key={`${data.root}/${data.versionId}`}
              />
            )}
          </>
        )}
      </PagerLayout>
    </div>
  );
}

/**
 * 「已安装」面板（侧边栏第三分区）：
 * 展示当前版本所在实例根目录下的本地资源（mods / 资源包 / 光影 / 存档），
 * 数据来自 `resources_scan`（仅本地文件系统，无外部请求）。
 */
function InstalledPanel({
  root,
  versionId,
}: {
  root: string;
  versionId: string;
}) {
  const {
    state,
    entries,
    issues,
    reload,
    root: scannedRoot,
  } = useResources("all", { root, versionId });
  const scanRoot = scannedRoot?.dir ?? "";
  const matchesRoot = scanRoot !== "";
  const reloadButton = (
    <Button variant="primary" iconLeft={<IconRefresh />} onClick={reload}>
      重新扫描
    </Button>
  );

  return (
    <div className="vst-installed">
      <header className="vst-installed__head">
        <h3 className="vst-installed__title">已安装</h3>
        <Button
          variant="secondary"
          size="sm"
          iconLeft={<IconRefresh />}
          loading={state === "loading"}
          onClick={reload}
        >
          重新扫描
        </Button>
      </header>

      {state === "loading" ? (
        <div className="vst-installed__loading">
          <div className="vst-installed__status">
            <span className="vst-loading__spinner" aria-hidden />
            <span>正在扫描本地资源...</span>
          </div>
          <ResourceSkeletonGrid />
        </div>
      ) : entries.length > 0 ? (
        <div className="vst-installed__grid" role="list">
          {entries.map((entry, index) => (
            <ResourceCard key={entry.id} entry={entry} index={index} />
          ))}
        </div>
      ) : (
        <ResourceNotice
          icon={<IconFolderOpen />}
          title={
            matchesRoot
              ? "该实例目录下没有可列出的资源"
              : "扫描目录与当前实例不一致"
          }
          hint={
            matchesRoot
              ? "把文件放进对应目录后，点击「重新扫描」即可在这里看到。"
              : `当前扫描的是 ${
                  scanRoot || "（未检测到实例目录）"
                }，与该版本的实例目录不一致。`
          }
          actions={reloadButton}
        />
      )}

      {state !== "loading" && issues.length > 0 ? (
        <ul className="vst-installed__issues">
          {issues.map((issue) => (
            <li key={issue}>{issue}</li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
