import { useEffect, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import logoUrl from "../assets/logo.png";
import aronaAvatar from "../assets/credits/itzarona.png";
import planaAvatar from "../assets/credits/itzplana.png";
import pclAvatar from "../assets/credits/pcl.png";
import pclCeAvatar from "../assets/credits/pcl-ce.png";
import linuxDoAvatar from "../assets/credits/linuxdo.png";
import shittimChestsAvatar from "../assets/credits/shittimchests.png";
import {
  Button,
  Card,
  Checkbox,
  Modal,
  PagerLayout,
  Switch,
  Tooltip,
  type PagerItem,
} from "../components/ui";
import { IconGlobe, IconNickel, IconSpark } from "../components/ui/icons";
import { showToast } from "../lib/toast";
import { formatBytes } from "../components/resources/format";
import {
  describeMemoryOptimizeError,
  fetchMemoryOptimizeCapability,
  runMemoryOptimize,
  type MemoryOptimizeCapability,
  type MemoryOptimizeReport,
} from "../lib/memoryOptimize";
import {
  cleanGameJunk,
  describeGameJunkError,
  scanGameJunk,
  type GameJunkScan,
} from "../lib/gameJunk";
import { describeJrrpError, fetchJrrp, type JrrpReport } from "../lib/jrrp";
import { isTauri, useSettings } from "../lib/settings";
import { setRoute } from "../lib/nav";
import {
  ASSETS_LICENSE,
  ASSETS_LICENSE_URL,
  CODE_LICENSE,
  CODE_LICENSE_URL,
  DEVELOPERS,
  PRODUCT_NAME,
  PRODUCT_VERSION,
  PROJECT_URL,
  SPECIAL_THANKS,
  type BrandCredit,
} from "../lib/brand";
import "./More.css";

const CREDIT_AVATARS: Readonly<Record<string, string>> = {
  itzarona: aronaAvatar,
  itzplana: planaAvatar,
  pcl: pclAvatar,
  "pcl-ce": pclCeAvatar,
  linuxdo: linuxDoAvatar,
  shittimchests: shittimChestsAvatar,
};

const SECTIONS: PagerItem[] = [
  { id: "guide", label: "指南", icon: <IconGlobe /> },
  { id: "frontier", label: "前沿计划", icon: <IconSpark /> },
  { id: "about", label: "关于", icon: <IconNickel /> },
];

const MEMORY_OPTIMIZE_ACTION_HELP =
  "把各个程序暂时用不到的内存回收给系统，通常需要几秒钟。Windows 会弹出管理员授权（UAC）；macOS / Linux 需要 sudo 或 su（root）权限。";

const MEMORY_PURGE_HELP =
  "额外清空系统的待命内存列表（被系统缓存的文件内容），可以腾出更多空闲内存；代价是这些文件下次需要重新从硬盘读取。仅 Windows 支持。";

const JRRP_HELP = "按本地日期生成 1~100 的稳定人品值，同一天内不会变化。";

const JUNK_CLEAN_HELP =
  "清理各个游戏目录的日志、崩溃报告、JVM 崩溃日志和启动器残留的临时文件。它们会在需要时自动重新生成，不会动到游戏本体、Mod、资源包和存档；清理前会先统计大小并让你确认。";

function GuideItem({ term, children }: { term: string; children: ReactNode }) {
  return (
    <li className="mo-item">
      <span className="mo-item__term">{term}</span>
      <span className="mo-item__text">{children}</span>
    </li>
  );
}

/** 每张指南卡只保留一个真实可达的页面入口。 */
function GuideCard({
  title,
  subtitle,
  actionLabel,
  onAction,
  children,
}: {
  title: string;
  subtitle: string;
  actionLabel: string;
  onAction: () => void;
  children: ReactNode;
}) {
  return (
    <Card
      className="mo-card"
      padding="lg"
      title={title}
      subtitle={subtitle}
      actions={
        <Button
          className="mo-card__action"
          variant="ghost"
          size="sm"
          onClick={onAction}
        >
          {actionLabel}
        </Button>
      }
    >
      <ul className="mo-list">{children}</ul>
    </Card>
  );
}

function Guide() {
  return (
    <div className="mo-page">
      <GuideCard
        title="账号"
        subtitle="三种账号形态共用首页顶部入口，登录与角色操作都在首页卡片内"
        actionLabel="回到首页登录"
        onAction={() => setRoute("home")}
      >
        <GuideItem term="正版（Microsoft）">
          在首页顶部切到「正版」后点「登录」，按弹窗给出的一次性设备码完成微软授权；账号会出现在首页卡片中。
        </GuideItem>
        <GuideItem term="皮肤站">
          切到「皮肤站」，确认服务器地址（默认{" "}
          <code>https://littleskin.cn</code>
          ），用皮肤站账号登录；账号下有多个角色时，在弹窗中选定本次使用的角色。
        </GuideItem>
        <GuideItem term="离线">
          在「设置 → 账号与隐私」设置离线用户名，再到首页切换为「离线」。
        </GuideItem>
        <GuideItem term="切换账号">
          在首页登录卡片的「切换账号」入口选择已保存账号，或使用「登录其他账号」添加账号。
        </GuideItem>
      </GuideCard>

      <GuideCard
        title="实例与版本"
        subtitle="目录、版本与启动选择集中在实例页"
        actionLabel="打开实例管理"
        onAction={() => setRoute("instances")}
      >
        <GuideItem term="管理目录">
          在实例页左侧新建、添加或导入 <code>.minecraft</code>{" "}
          目录；选中目录后，右侧列出该目录下已安装的版本。
        </GuideItem>
        <GuideItem term="选择启动实例">
          点整张实例卡即选中该版本；选中后自动回到首页，卡片标记「当前启动」，首页「启动游戏」使用该实例。
        </GuideItem>
        <GuideItem term="收藏与更多">
          心形按钮用于收藏；卡片右下角直接提供「删除实例」和「版本设置」，删除前会再次确认。
        </GuideItem>
        <GuideItem term="内存上限">
          「设置 → 游戏 → 内存分配」设置全局默认；在实例卡「实例设置 →
          设置」中可为当前实例覆盖。
        </GuideItem>
      </GuideCard>

      <GuideCard
        title="下载与来源"
        subtitle="游戏和社区资源在资源页；来源和限速在设置中"
        actionLabel="打开资源页"
        onAction={() => setRoute("resources")}
      >
        <GuideItem term="原版与加载器">
          在「资源 → 原版游戏」选择 Minecraft 版本和需要的加载器，再开始安装。
        </GuideItem>
        <GuideItem term="社区资源">
          Mod、资源包和光影选择版本后另存到目标目录；整合包输入实例名称后创建独立实例。
        </GuideItem>
        <GuideItem term="来源策略">
          在「设置 →
          下载」分别调整版本列表源、文件下载源和社区资源源。三项策略彼此独立。
        </GuideItem>
        <GuideItem term="下载很慢">
          在「设置 →
          下载」把文件下载源改为镜像优先，并检查「速度限制」与「最大线程数」；列表加载慢时单独调整版本列表源。
        </GuideItem>
      </GuideCard>

      <GuideCard
        title="外观与排障"
        subtitle="外观修改即时生效；启动与账号问题按实际入口排查"
        actionLabel="打开设置"
        onAction={() => setRoute("settings")}
      >
        <GuideItem term="明暗与颜色">
          明暗模式决定界面亮暗（跟随系统 / 亮色 /
          暗色）；颜色主题决定按钮、选中态与焦点环的信号色。二者独立设置。
        </GuideItem>
        <GuideItem term="背景">
          在「设置 → 外观 →
          背景图片」选择本地图片，并调整背景模糊度与遮罩浓度；底部实时预览会同步变化。
        </GuideItem>
        <GuideItem term="启动按钮是灰的">
          先在实例页选择一个版本；离线用户名在「设置 → 账号与隐私」填写。
        </GuideItem>
        <GuideItem term="邮箱与头像">
          在「设置 →
          账号与隐私」调整隐私模式；正版账号的角色入口打开皮肤页，皮肤站账号可在角色入口重新选择角色。
        </GuideItem>
      </GuideCard>
    </div>
  );
}

/** 前沿计划：默认关闭的试验性能力；开关状态写入 settings.json。 */
function Frontier() {
  const { settings, update } = useSettings();
  const namingHelp =
    "添加的目录名为 .minecraft，且上级文件夹仅有一个可识别实例目录时，可用上级文件夹名作为显示名。磁盘根、用户目录和系统目录不参与自动取名。启动器自带目录没有自定义标签时显示为「默认实例目录」。";

  return (
    <div className="mo-page">
      <Card className="mo-card mo-frontier__notice" padding="lg">
        <div className="mo-frontier__notice-body">
          <span className="mo-frontier__notice-icon" aria-hidden="true">
            <IconSpark />
          </span>
          <p>
            本页的内容属于“前沿计划”(Frontier
            Project)：这些功能仍在试验阶段，行为可能在后续版本调整。
          </p>
        </div>
      </Card>

      <StarlightLibrary />

      <Card className="mo-card" padding="lg" title="用户界面项">
        <div className="mo-frontier__row">
          <div className="mo-frontier__row-label">
            <strong>顶栏颜色小改</strong>
            <span>让半透明顶栏轻微跟随主题色。</span>
          </div>
          <Switch
            aria-label="顶栏颜色小改"
            checked={settings.titlebarTint}
            onChange={(titlebarTint) => update({ titlebarTint })}
          />
        </div>
      </Card>

      <Card
        className="mo-card"
        padding="lg"
        title="操作体验项"
        subtitle="可能改变默认操作习惯的试验性改进"
      >
        <div className="mo-frontier__row">
          <div className="mo-frontier__row-label">
            <strong>实例取名优化</strong>
            <span>添加孤立 .minecraft 时，用上级文件夹名作为显示名。</span>
          </div>
          <Tooltip label={namingHelp}>
            <Switch
              aria-label="实例取名优化"
              checked={settings.instanceNamingOptimize}
              onChange={(instanceNamingOptimize) =>
                update({ instanceNamingOptimize })
              }
            />
          </Tooltip>
        </div>
      </Card>
    </div>
  );
}

/** 内存回收按平台申请管理员权限；密码仅用于本次后端命令。 */
function StarlightLibrary() {
  const { settings, update } = useSettings();
  const [capability, setCapability] = useState<MemoryOptimizeCapability | null>(
    null
  );
  const [running, setRunning] = useState(false);
  const [passwordOpen, setPasswordOpen] = useState(false);
  const [password, setPassword] = useState("");
  const [jrrp, setJrrp] = useState<JrrpReport | null>(null);
  const [jrrpOpen, setJrrpOpen] = useState(false);
  const [jrrpLoading, setJrrpLoading] = useState(false);
  const [junk, setJunk] = useState<GameJunkScan | null>(null);
  const [junkOpen, setJunkOpen] = useState(false);
  const [junkRunning, setJunkRunning] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void fetchMemoryOptimizeCapability()
      .then((value) => {
        if (!cancelled) setCapability(value);
      })
      .catch(() => {
        // 浏览器预览等非 Tauri 环境：保持 null；点击时仍会尝试调用命令并报错。
        if (!cancelled) setCapability(null);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // 能力尚未返回时用 UA 兜底，避免 Windows 上分页回收开关先闪一下禁用态。
  const guessedWindows =
    typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
  // 以能力返回的 supportsStandbyPurge 为准；未知时才退回平台猜测。
  const standbySupported = capability?.supportsStandbyPurge ?? guessedWindows;

  function present(report: MemoryOptimizeReport) {
    if (report.status === "success") {
      const freed = Math.max(0, report.availableDeltaBytes);
      showToast(
        freed >= 16 * 1024 * 1024
          ? `内存优化完成，释放了 ${formatBytes(freed)} 内存`
          : "内存优化完成，当前没有多少可以释放的内存",
        "success"
      );
      return;
    }
    if (report.status === "busy") {
      showToast("内存优化正在进行中，请稍等一下", "info");
      return;
    }
    showToast(
      report.message ??
        (report.status === "permissionDenied"
          ? "已取消管理员授权，内存优化未执行"
          : "内存优化失败"),
      "error"
    );
  }

  async function execute(withPassword?: string) {
    if (running) return;
    setRunning(true);
    try {
      const purge = standbySupported && settings.memoryPurgeStandby;
      present(await runMemoryOptimize(purge, withPassword));
    } catch (error) {
      showToast(`内存优化失败：${describeMemoryOptimizeError(error)}`, "error");
    } finally {
      setRunning(false);
    }
  }

  function start() {
    if (running) return;
    if (capability && !capability.available) {
      showToast(capability.message ?? "无法运行完整内存优化", "error");
      return;
    }
    if (capability?.needsPassword) {
      setPassword("");
      setPasswordOpen(true);
      return;
    }
    void execute();
  }

  function closePassword() {
    setPasswordOpen(false);
    setPassword("");
  }

  function submitPassword() {
    if (!password || running) return;
    const value = password;
    closePassword();
    void execute(value);
  }

  async function showJrrp() {
    if (jrrpLoading) return;
    setJrrpLoading(true);
    try {
      const report = await fetchJrrp();
      setJrrp(report);
      setJrrpOpen(true);
    } catch (error) {
      showToast(`今日人品计算失败：${describeJrrpError(error)}`, "error");
    } finally {
      setJrrpLoading(false);
    }
  }

  async function prepareJunk() {
    if (junkRunning) return;
    setJunkRunning(true);
    try {
      const scan = await scanGameJunk();
      if (scan.fileCount === 0 || scan.totalBytes === 0) {
        showToast("没有找到可以清理的垃圾文件", "success");
        return;
      }
      setJunk(scan);
      setJunkOpen(true);
    } catch (error) {
      showToast(`统计垃圾文件失败：${describeGameJunkError(error)}`, "error");
    } finally {
      setJunkRunning(false);
    }
  }

  async function runJunkClean() {
    if (junkRunning) return;
    setJunkRunning(true);
    try {
      const result = await cleanGameJunk();
      setJunkOpen(false);
      if (result.removedCount === 0) {
        showToast("没有文件被清理，它们可能正被游戏占用", "error");
      } else if (result.failedCount > 0) {
        showToast(
          `清理完成，腾出了 ${formatBytes(result.freedBytes)}；${
            result.failedCount
          } 个文件正被占用`,
          "info"
        );
      } else {
        showToast(
          `清理完成，腾出了 ${formatBytes(result.freedBytes)} 空间`,
          "success"
        );
      }
    } catch (error) {
      showToast(`清理游戏垃圾失败：${describeGameJunkError(error)}`, "error");
    } finally {
      setJunkRunning(false);
    }
  }

  return (
    <Card className="mo-card" padding="lg" title="星月图书馆">
      <div className="mo-starlight">
        <div className="mo-starlight__actions">
          <Tooltip label={JRRP_HELP}>
            <Button
              variant="secondary"
              disabled={jrrpLoading}
              onClick={() => void showJrrp()}
            >
              今日人品
            </Button>
          </Tooltip>
          <Tooltip label={MEMORY_OPTIMIZE_ACTION_HELP}>
            <Button variant="primary" disabled={running} onClick={start}>
              {running ? "优化中……" : "内存优化"}
            </Button>
          </Tooltip>
          <Tooltip label={JUNK_CLEAN_HELP}>
            <Button
              variant="danger"
              disabled={junkRunning}
              onClick={() => void prepareJunk()}
            >
              {junkRunning ? "处理中……" : "清理游戏垃圾"}
            </Button>
          </Tooltip>
        </div>
        <Tooltip label={MEMORY_PURGE_HELP}>
          <Checkbox
            checked={standbySupported && settings.memoryPurgeStandby}
            disabled={!standbySupported}
            onChange={(checked) => update({ memoryPurgeStandby: checked })}
            label="内存优化时清理待命内存列表"
          />
        </Tooltip>
      </div>

      <Modal
        open={passwordOpen}
        onClose={closePassword}
        title="需要管理员权限"
        description="完整内存优化需要 sudo 或 su（root）权限。"
        width={420}
        footer={
          <>
            <Button variant="ghost" size="sm" onClick={closePassword}>
              取消
            </Button>
            <Button
              variant="primary"
              size="sm"
              disabled={!password || running}
              onClick={submitPassword}
            >
              开始优化
            </Button>
          </>
        }
      >
        <label className="mo-starlight__field">
          <span>sudo 密码或 su（root）密码</span>
          <input
            className="mo-starlight__input"
            type="password"
            value={password}
            autoComplete="off"
            spellCheck={false}
            placeholder="输入密码后继续"
            onChange={(event) => setPassword(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                submitPassword();
              }
            }}
          />
        </label>
      </Modal>

      <Modal
        open={jrrpOpen}
        onClose={() => setJrrpOpen(false)}
        title="今日人品"
        width={380}
        footer={
          <Button
            variant="primary"
            size="sm"
            onClick={() => setJrrpOpen(false)}
          >
            确定
          </Button>
        }
      >
        {jrrp ? (
          <div className="mo-jrrp">
            <div className="mo-jrrp__value">{jrrp.value}</div>
            <p className="mo-jrrp__comment">{jrrp.comment}</p>
          </div>
        ) : null}
      </Modal>

      <Modal
        open={junkOpen}
        onClose={() => {
          if (!junkRunning) setJunkOpen(false);
        }}
        title="清理游戏垃圾"
        width={440}
        footer={
          <>
            <Button
              variant="ghost"
              size="sm"
              disabled={junkRunning}
              onClick={() => setJunkOpen(false)}
            >
              取消
            </Button>
            <Button
              variant="danger"
              size="sm"
              disabled={junkRunning}
              onClick={() => void runJunkClean()}
            >
              {junkRunning ? "清理中……" : "清理"}
            </Button>
          </>
        }
      >
        <div className="mo-junk">
          <p className="mo-junk__text">
            找到 {junk?.fileCount ?? 0} 个日志、崩溃报告或临时文件，约{" "}
            {formatBytes(junk?.totalBytes ?? 0)}。
          </p>
          <p className="mo-junk__hint">
            它们会在需要时自动重新生成，不会动到游戏本体、Mod、资源包和存档。
          </p>
        </div>
      </Modal>
    </Card>
  );
}

function CreditCard({
  credit,
  onOpen,
}: {
  credit: BrandCredit;
  onOpen: (url: string) => void;
}) {
  const avatar = CREDIT_AVATARS[credit.id];
  const url = credit.url;

  return (
    <article
      className={`mo-credit${avatar ? "" : " mo-credit--text"}`}
      aria-label={credit.name}
    >
      {avatar ? (
        <img
          className="mo-credit__avatar"
          src={avatar}
          alt={`${credit.name} 头像`}
          width={44}
          height={44}
          draggable={false}
        />
      ) : null}
      <h4 className="mo-credit__name">{credit.name}</h4>
      {url ? (
        <Button
          className="mo-credit__link"
          variant="ghost"
          size="sm"
          aria-label={`打开 ${credit.name} 的${credit.linkLabel}`}
          data-brand-url={url}
          onClick={() => onOpen(url)}
        >
          {credit.linkLabel}
        </Button>
      ) : null}
    </article>
  );
}

function About() {
  const [error, setError] = useState("");

  async function openExternal(url: string) {
    setError("");
    try {
      if (isTauri()) {
        await openUrl(url);
      } else {
        window.open(url, "_blank", "noopener,noreferrer");
      }
    } catch {
      setError(`无法打开页面，请在浏览器地址栏输入 ${url}`);
    }
  }

  return (
    <div className="mo-page mo-about">
      <Card className="mo-card mo-about__intro" padding="lg">
        <div className="mo-about__brand">
          <img
            className="mo-about__logo"
            src={logoUrl}
            alt={`${PRODUCT_NAME} 应用徽标`}
            width={64}
            height={64}
            draggable={false}
          />
          <div className="mo-about__identity">
            <h2 className="mo-about__name">{PRODUCT_NAME}</h2>
            <p className="mo-about__description">
              跨平台的 Minecraft Java 版启动器。
            </p>
          </div>
          <span
            className="mo-about__ver"
            aria-label={`当前版本 ${PRODUCT_VERSION}`}
          >
            v{PRODUCT_VERSION}
          </span>
        </div>
        <div className="mo-about__meta">
          <Button
            variant="secondary"
            size="sm"
            icon="globe"
            aria-label={`打开 ${PRODUCT_NAME} 项目主页`}
            data-brand-url={PROJECT_URL}
            onClick={() => void openExternal(PROJECT_URL)}
          >
            项目主页
          </Button>
          <dl className="mo-about__licenses">
            <div>
              <dt>代码</dt>
              <dd>
                <Button
                  variant="ghost"
                  size="sm"
                  aria-label="查看代码许可 AGPL-3.0-only"
                  data-brand-url={CODE_LICENSE_URL}
                  onClick={() => void openExternal(CODE_LICENSE_URL)}
                >
                  {CODE_LICENSE}
                </Button>
              </dd>
            </div>
            <div>
              <dt>原创美术</dt>
              <dd>
                <Button
                  variant="ghost"
                  size="sm"
                  aria-label="查看原创美术许可 CC-BY-SA-4.0"
                  data-brand-url={ASSETS_LICENSE_URL}
                  onClick={() => void openExternal(ASSETS_LICENSE_URL)}
                >
                  {ASSETS_LICENSE}
                </Button>
              </dd>
            </div>
          </dl>
        </div>
      </Card>

      {error ? (
        <p className="mo-error" role="alert">
          {error}
        </p>
      ) : null}

      <section
        className="mo-about__section"
        aria-labelledby="mo-developers-title"
      >
        <h3 id="mo-developers-title" className="mo-about__section-title">
          开发者
        </h3>
        <div className="mo-about__credits">
          {DEVELOPERS.map((credit) => (
            <CreditCard
              key={credit.id}
              credit={credit}
              onOpen={(url) => void openExternal(url)}
            />
          ))}
        </div>
      </section>

      <section className="mo-about__section" aria-labelledby="mo-thanks-title">
        <h3 id="mo-thanks-title" className="mo-about__section-title">
          特别鸣谢
        </h3>
        <div className="mo-about__credits">
          {SPECIAL_THANKS.map((credit) => (
            <CreditCard
              key={credit.id}
              credit={credit}
              onOpen={(url) => void openExternal(url)}
            />
          ))}
        </div>
      </section>
    </div>
  );
}

/** 更多页：左侧分区切换器（指南 / 前沿计划 / 关于），右侧各自独立成页。 */
export default function More() {
  const [active, setActive] = useState<string>(SECTIONS[0].id);

  return (
    <PagerLayout
      className="nw-pager--fill"
      items={SECTIONS}
      active={active}
      onSelect={setActive}
      navLabel="更多分区"
      navTitle="更多"
    >
      {active === "guide" ? (
        <Guide />
      ) : active === "frontier" ? (
        <Frontier />
      ) : (
        <About />
      )}
    </PagerLayout>
  );
}
