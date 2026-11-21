import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useSettings } from "../lib/settings";
import { openVersionSettings, setRoute } from "../lib/nav";
import { useLaunchInstance } from "../lib/instances";
import {
  Button,
  Card,
  IconButton,
  IconPencil,
  IconSwap,
  Tooltip,
} from "../components/ui";
import { IconLayers, IconSliders } from "../components/account/icons";
import { InlineNotice } from "../components/ui/InlineNotice";
import { IconAlert } from "../components/resources/icons";
import { AccountPanel } from "../components/account/AccountPanel";

import { AccountTypeTabs } from "../components/account/AccountTypeTabs";
import { CharacterAvatar } from "../components/account/CharacterAvatar";
import { SkinLoginDialog } from "../components/account/SkinLoginDialog";
import { SkinProfileDialog } from "../components/account/SkinProfileDialog";
import { AccountSwitcherModal } from "../components/account/AccountSwitcherModal";
import { CharacterPickerModal } from "../components/account/CharacterPickerModal";
import {
  getSkinSession,
  setSkinSession,
  writeCachedProfiles,
  type SkinAccount,
  type SkinProfile,
  type SkinSession,
} from "../components/account/yggdrasil";
import {
  maskEmail,
  type Account,
  type AccountType,
  type Identity,
} from "../components/account/types";
import { useAccounts } from "../components/account/useAccounts";
import { LaunchButton } from "../components/launch/LaunchButton";
import {
  LaunchErrorDialog,
  LaunchPanel,
  launchErrorInfo,
} from "../components/launch/LaunchPanel";
import { useLauncher } from "../components/launch/useLauncher";
import "./Home.css";

const AVATAR_SIZE = 72;

const EMPTY_IDENTITY: Identity = { name: "", email: "", loggedIn: false };

/** 账号写入（accounts_upsert）与激活（accounts_set_active）的结果。 */
interface PersistResult {
  /** 账号列表是否成功写入本地配置 */
  persisted: boolean;
  /** 当前账号是否成功激活 */
  activated: boolean;
}

/** 持久化失败提示的三种组合。 */
interface PersistMessages {
  writeFailed: string;
  activateFailed: string;
  bothFailed: string;
}

/**
 * 账号持久化提示：两项都成功时返回空字符串，用于清除旧提示。
 * 微软登录成功时账号已由后端 msa_device_poll 落库，因此提示只描述
 * 「本地同步 / 激活未写入」及可执行的修复动作，不说需要重新登录。
 */
function persistNotice(
  result: PersistResult,
  messages: PersistMessages
): string {
  if (!result.persisted && !result.activated) return messages.bothFailed;
  if (!result.persisted) return messages.writeFailed;
  if (!result.activated) return messages.activateFailed;
  return "";
}

const MICROSOFT_PERSIST_MESSAGES: PersistMessages = {
  writeFailed: "账号已登录，但本地账号信息未能写入；请检查配置目录写入权限。",
  activateFailed:
    "账号已登录；激活状态未能写入，重启后可能需要在「切换账号」中重新选择。",
  bothFailed:
    "账号已登录，但本地账号信息与激活状态均未能写入；请检查配置目录写入权限，并在「切换账号」中重新选择账号。",
};

const SKIN_PERSIST_MESSAGES: PersistMessages = {
  writeFailed: "角色信息未能写入本地配置；请检查配置目录写入权限。",
  activateFailed:
    "角色已切换，但激活状态未能写入；重启后可能需要在「切换账号」中重新选择。",
  bothFailed:
    "角色信息与激活状态均未能写入本地配置；请检查配置目录写入权限，并在「切换角色」中重新选择。",
};

/**
 * 剥离皮肤站地址末尾的 Yggdrasil API 根，保留站点子目录。
 *
 * 只处理路径部分，主机名不会被误伤：
 *   https://example.com/mc/yggdrasil     → https://example.com/mc
 *   https://example.com/mc/api/yggdrasil → https://example.com/mc
 *   https://example.com/api/yggdrasil    → https://example.com
 *   https://yggdrasil                    → https://yggdrasil
 */
function stripYggdrasilRoot(root: string): string {
  const match = /^(https?:\/\/[^/]+)(\/.*)?$/i.exec(root);
  if (!match) return root;
  const origin = match[1];
  const path = (match[2] ?? "").replace(/\/+$/, "");
  const sitePath = path
    .replace(/\/api\/yggdrasil$/i, "")
    .replace(/\/yggdrasil$/i, "");
  return `${origin}${sitePath}`;
}

/** 与共享 AnimatedSwap 同一降级策略：订阅系统「减弱动效」偏好。 */
const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

function subscribeReducedMotion(onChange: () => void) {
  const media = window.matchMedia(REDUCED_MOTION_QUERY);
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}

function getReducedMotion() {
  return window.matchMedia(REDUCED_MOTION_QUERY).matches;
}

/**
 * 模块切换动画：值变化时先「淡出」，替换内容后再「淡入 + 轻微上移」，
 * 避免瞬间硬切。返回用于渲染的 key 与当前阶段。
 */
function useAnimatedSwap(key: string, duration = 170) {
  const [display, setDisplay] = useState(key);
  const [phase, setPhase] = useState<"in" | "out">("in");
  const reducedMotion = useSyncExternalStore(
    subscribeReducedMotion,
    getReducedMotion
  );

  useEffect(() => {
    if (key === display) return;
    // 减弱动效时不播淡出、也不等计时器：CSS 已关闭过渡，再等 170ms 会让
    // 账号区先空一帧再出现，直接替换内容才是正确的降级。
    if (reducedMotion) {
      setDisplay(key);
      setPhase("in");
      return;
    }
    setPhase("out");
    const timer = window.setTimeout(() => {
      setDisplay(key);
      setPhase("in");
    }, duration);
    return () => window.clearTimeout(timer);
  }, [key, display, duration, reducedMotion]);

  return { display, phase };
}

export function Home() {
  const { settings, update } = useSettings();
  const accountType = settings.accountType;

  // ---- 多账号 ----
  const { accounts, activeId, upsert, remove, setActive } = useAccounts();

  // ---- 启动实例（实例与版本页选择，首页只消费） ----
  const { instance } = useLaunchInstance();

  // ---- 启动流程 ----
  const launcher = useLauncher();

  // ---- 弹窗 ----

  const [skinDialogOpen, setSkinDialogOpen] = useState(false);
  const [profileSwitch, setProfileSwitch] = useState<SkinSession | null>(null);
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [linkError, setLinkError] = useState("");
  /** 账号写入 / 激活失败的可执行提示；成功与退出、切换账号时清空。 */
  const [accountNotice, setAccountNotice] = useState("");
  /** 账号形态切换中：激活命令完成前禁用启动，避免沿用旧身份启动。 */
  const [accountSwitching, setAccountSwitching] = useState(false);

  // 头像刷新信号（皮肤上传 / 改名后强制重新解析）。
  const [avatarNonce, setAvatarNonce] = useState(0);

  // 当前激活账号（仅当其类型与当前形态一致时生效）。
  const activeAccount = useMemo<Account | null>(() => {
    return accounts.find((a) => a.id === activeId) ?? null;
  }, [accounts, activeId]);

  const typedAccount = useMemo<Account | null>(
    () =>
      activeAccount && activeAccount.type === accountType
        ? activeAccount
        : null,
    [activeAccount, accountType]
  );

  const offlineName = typedAccount?.name ?? settings.offlineUsername ?? "";

  // 按账号形态派生最终身份。
  const effectiveIdentity = useMemo<Identity>(() => {
    if (typedAccount) {
      return {
        name: typedAccount.name || "Player",
        email: typedAccount.email ?? "",
        loggedIn: true,
      };
    }
    if (accountType === "offline") {
      return {
        name: offlineName.trim() || "Player",
        email: "",
        loggedIn: true,
      };
    }
    return EMPTY_IDENTITY;
  }, [typedAccount, accountType, offlineName]);

  const launchUsername = effectiveIdentity.name;
  const hasEmail = effectiveIdentity.email.trim() !== "";
  const emailMasked = Boolean(settings.privacyMode) && hasEmail;
  const emailText = hasEmail
    ? emailMasked
      ? maskEmail(effectiveIdentity.email)
      : effectiveIdentity.email
    : "尚未绑定邮箱";

  // ---- 处理器 ----
  /**
   * 写入账号列表并激活：两个命令失败时返回 false 而非抛错，这里仍自行兜住
   * 异常，保证永不 reject，调用方只需根据结果决定提示。
   */
  const persistAccount = useCallback(
    async (account: Account): Promise<PersistResult> => {
      let persisted = false;
      try {
        persisted = await upsert(account);
      } catch {
        persisted = false;
      }
      // 即使账号列表同步失败也继续激活：微软登录已由 msa_device_poll 落库，
      // 激活通常仍能成功，不应因此放弃当前账号。
      let activated = false;
      try {
        activated = await setActive(account.id);
      } catch {
        activated = false;
      }
      return { persisted, activated };
    },
    [upsert, setActive]
  );

  const handleAccountTypeChange = useCallback(
    (value: string) => {
      const next = value as AccountType;
      update({ accountType: next });
      // 切换形态即离开原账号语境，旧的持久化提示不再适用。
      setAccountNotice("");
      // 离线形态没有后端账号，清掉 active 避免继续沿用上一次的在线身份。
      const nextActive =
        next === "offline"
          ? ""
          : accounts.find((a) => a.type === next)?.id ?? "";
      // 激活命令完成前置为切换中，启动按钮暂时不可用。
      setAccountSwitching(true);
      void setActive(nextActive).finally(() => setAccountSwitching(false));
    },
    [update, accounts, setActive]
  );

  /**
   * 微软设备码登录成功：后端 msa_device_poll 已把权威账号落库。
   * upsert 会再次调用 accounts_upsert 同步前端列表，由后端按 id 合并、
   * 保留 access_token / 密码等敏感字段；这里不再拼同名影子账号。
   *
   * 两个命令失败时都不会抛错，而是返回布尔值；本函数必须自行兜住所有失败
   * 且永不 reject，否则登录组件里的错误提示不可达。即使 upsert 失败也要
   * 继续激活，避免把已成功登录的账号误报为未登录。
   */
  const applyMicrosoftAccount = useCallback(
    async (account: Account) => {
      const result = await persistAccount({
        id: account.id,
        type: "microsoft",
        name: account.name || "Player",
        uuid: account.uuid,
        email: account.email,
        server: undefined,
      });
      // 写入与激活独立判断；两项都成功时返回空字符串，清除旧提示。
      setAccountNotice(persistNotice(result, MICROSOFT_PERSIST_MESSAGES));
      setAvatarNonce((n) => n + 1);
    },
    [persistAccount]
  );

  /** 皮肤站多角色：后端返回的权威账号落库并设为当前账号。 */
  const applySkinAccount = useCallback(
    async (account: SkinAccount) => {
      const result = await persistAccount({
        id: account.id,
        type: "littleskin",
        name: account.name || "Player",
        uuid: account.uuid ?? undefined,
        email: account.email ?? undefined,
        server: account.server ?? settings.littleskinServer,
      });
      // 持久化失败不阻断登录结果展示，只用可执行提示告知用户如何处理。
      setAccountNotice(persistNotice(result, SKIN_PERSIST_MESSAGES));
      // 登录时返回的角色列表只在这一次可用：缓存下来供「切换角色」即时渲染。
      const session = getSkinSession();
      const profiles: SkinProfile[] =
        session?.profiles ??
        (account.uuid
          ? [{ id: account.uuid.replace(/-/g, ""), name: account.name }]
          : []);
      writeCachedProfiles(account.id, profiles);
      setAvatarNonce((n) => n + 1);
    },
    [persistAccount, settings.littleskinServer]
  );

  /** 皮肤站返回多角色：统一在首页弹出单选弹窗（若登录发生在弹窗里，先关掉那个弹窗）。 */
  const handleProfileSelect = useCallback((session: SkinSession) => {
    setSkinDialogOpen(false);
    setProfileSwitch(session);
  }, []);

  const handleLogout = useCallback(() => {
    // 退出后旧的「激活状态未能写入」等提示不再适用。
    setAccountNotice("");
    setSkinSession(null);
    // 激活命令完成前置为切换中，避免「退出后立刻启动」仍沿用旧账号身份。
    setAccountSwitching(true);
    void setActive("").finally(() => setAccountSwitching(false));
  }, [setActive]);

  /** 切换账号：选中另一个已保存账号；形态不同时一并切换胶囊形态。 */
  const handleSwitchAccount = useCallback(
    (id: string) => {
      // 重复点当前账号不做任何状态重排，避免头像被无谓刷新。
      if (id === activeId) return;
      const target = accounts.find((account) => account.id === id);
      if (!target) return;
      // 切换账号后旧账号的持久化提示不应残留。
      setAccountNotice("");
      if (target.type !== accountType) update({ accountType: target.type });
      // 激活命令完成前置为切换中，避免「切账号后立刻启动」沿用旧账号身份。
      setAccountSwitching(true);
      void setActive(id).finally(() => setAccountSwitching(false));
      setSkinSession(null);
      setAvatarNonce((n) => n + 1);
    },
    [accounts, activeId, accountType, update, setActive]
  );

  /** 列表里没有想要的账号：退出当前账号，回到同形态的登录表单。 */
  const handleLoginOther = useCallback(() => {
    setSwitcherOpen(false);
    handleLogout();
  }, [handleLogout]);

  /** 铅笔：正版去官方皮肤页、皮肤站打开角色列表（离线形态不渲染铅笔）。 */
  const handlePencil = useCallback(async () => {
    setLinkError("");
    if (accountType === "microsoft") {
      try {
        await openUrl(
          "https://www.minecraft.net/zh-hans/msaprofile/mygames/editprofile"
        );
      } catch {
        setLinkError("无法打开浏览器，请手动访问 minecraft.net 编辑皮肤");
      }
      return;
    }
    if (accountType === "littleskin") {
      setPickerOpen(true);
    }
  }, [accountType]);

  /**
   * 皮肤站角色页：站点根可能只写了域名，统一补协议；
   * 若配置的是 Yggdrasil API 根，先回退到站点根（保留子目录）再拼路径。
   */
  const skinSiteUrl = useMemo(() => {
    const raw = (
      typedAccount?.server ??
      settings.littleskinServer ??
      ""
    ).trim();
    const site = raw.replace(/\/+$/, "") || "https://littleskin.cn";
    const root = /^https?:\/\//i.test(site) ? site : `https://${site}`;
    return `${stripYggdrasilRoot(root)}/user/player`;
  }, [typedAccount?.server, settings.littleskinServer]);

  const handleOpenSkinSite = useCallback(async () => {
    setLinkError("");
    try {
      await openUrl(skinSiteUrl);
    } catch {
      setLinkError("无法打开浏览器，请手动访问皮肤站角色页");
    }
  }, [skinSiteUrl]);

  /** 切换角色：用后端待选会话绑定令牌，不会新建重复账号。 */
  const handleSwitchProfile = useCallback(
    async (profile: SkinProfile, sessionId: string) => {
      const updated = await invoke<SkinAccount>("yggdrasil_select_profile", {
        sessionId,
        profileId: profile.id,
      });
      const result = await persistAccount({
        id: updated.id,
        type: "littleskin",
        name: updated.name || profile.name,
        uuid: updated.uuid ?? undefined,
        email: updated.email ?? typedAccount?.email ?? undefined,
        server: updated.server ?? typedAccount?.server ?? undefined,
      });
      setAccountNotice(persistNotice(result, SKIN_PERSIST_MESSAGES));
      setAvatarNonce((n) => n + 1);
    },
    [typedAccount, persistAccount]
  );

  const canLaunch =
    instance !== null &&
    launchUsername.trim() !== "" &&
    !launcher.running &&
    !accountSwitching;

  /** 启动失败只在弹窗里出现一次；诊断优先于普通错误。 */
  const launchError = launchErrorInfo(launcher.error, launcher.diagnostic);

  const handleLaunch = useCallback(() => {
    if (!canLaunch || !instance) return;
    void launcher.launch({
      versionId: instance.versionId,
      gameDir: instance.root,
      username: launchUsername.trim(),
      memoryMb: settings.memoryMb ?? 4096,
      source: settings.fileDownloadSource,
    });
  }, [
    canLaunch,
    instance,
    launcher,
    launchUsername,
    settings.memoryMb,
    settings.fileDownloadSource,
  ]);

  // 保留失败时的实例，不跟随之后在侧边栏选择的新实例。
  const reportTarget =
    launcher.reportTarget ??
    (instance
      ? { versionId: instance.versionId, gameDir: instance.root }
      : null);
  const handleOpenLogs = useCallback(() => {
    if (reportTarget) void launcher.openLogs(reportTarget);
  }, [reportTarget, launcher.openLogs]);
  const handleExportReport = useCallback(() => {
    if (!reportTarget)
      return Promise.reject(new Error("请返回实例列表重新选择失败的实例。"));
    return launcher.exportErrorReport(reportTarget);
  }, [reportTarget, launcher.exportErrorReport]);

  const accountForm = (
    <AccountPanel
      accountType={accountType}
      identity={effectiveIdentity}
      littleskinServer={settings.littleskinServer ?? ""}
      onMicrosoftAccount={applyMicrosoftAccount}
      onSkinAccount={applySkinAccount}
      onLogout={handleLogout}
      onProfileSelect={handleProfileSelect}
    />
  );

  // 角色区 / 表单 —— 账号形态或登录态变化时淡出→淡入。
  const swapKey = `${accountType}:${effectiveIdentity.loggedIn ? "in" : "out"}`;
  const { display: swapDisplay, phase: swapPhase } = useAnimatedSwap(swapKey);

  const isOffline = accountType === "offline";
  const loggedIn = isOffline || effectiveIdentity.loggedIn;
  const pencilLabel =
    accountType === "microsoft" ? "打开官方皮肤页" : "切换角色";

  const body = loggedIn ? (
    <div className="home__character">
      <div className="home__avatar-row">
        {!isOffline ? (
          <IconButton
            label={pencilLabel}
            className="home__avatar-action"
            onClick={(event) => {
              if (event.detail > 0) event.currentTarget.blur();
              void handlePencil();
            }}
          >
            <IconPencil />
          </IconButton>
        ) : null}
        <CharacterAvatar
          name={effectiveIdentity.name}
          size={AVATAR_SIZE}
          source={accountType}
          uuid={typedAccount?.uuid}
          server={typedAccount?.server ?? settings.littleskinServer}
          reloadKey={avatarNonce}
        />
        {!isOffline ? (
          <IconButton
            label="切换账号"
            className="home__avatar-action"
            onClick={(event) => {
              if (event.detail > 0) event.currentTarget.blur();
              setSwitcherOpen(true);
            }}
          >
            <IconSwap />
          </IconButton>
        ) : null}
      </div>

      <div className="home__identity">
        <div className="home__identity-text">
          <Tooltip label={effectiveIdentity.name || "未命名"}>
            <h2 className="home__name">{effectiveIdentity.name || "未命名"}</h2>
          </Tooltip>
          {!isOffline ? <p className="home__email">{emailText}</p> : null}
        </div>
      </div>

      {linkError ? (
        <InlineNotice tone="warning" icon={<IconAlert />} role="status">
          {linkError}
        </InlineNotice>
      ) : null}
    </div>
  ) : (
    <div className="home__form">{accountForm}</div>
  );

  return (
    <div className="home">
      <Card className="home__card" padding="lg">
        {/* 顶部：账号形态胶囊（全应用唯一形态切换入口） */}
        <div
          className="home__top home__stage home__stage--1"
          inert={launcher.starting}
        >
          <AccountTypeTabs
            className="home__accounts"
            value={accountType}
            onChange={handleAccountTypeChange}
          />
        </div>

        {/* 中部：角色区 / 未登录表单 */}
        <div className="home__body home__stage home__stage--2">
          <div className="home__presentation" data-starting={launcher.starting}>
            <div
              className="home__account"
              inert={launcher.starting}
              aria-hidden={launcher.starting}
            >
              <div
                className="home__swap"
                key={swapDisplay}
                data-phase={swapPhase}
              >
                {body}
              </div>
            </div>
            <div
              className="home__launch-progress"
              inert={!launcher.starting}
              aria-hidden={!launcher.starting}
            >
              {launcher.starting ? (
                <LaunchPanel
                  instanceName={
                    instance
                      ? instance.displayName || instance.versionId
                      : "未选择实例"
                  }
                  stage={launcher.stage}
                  progress={launcher.progress}
                  percent={launcher.percent}
                  speedBps={launcher.speedBps}
                  cancelRequested={launcher.cancelRequested}
                  onCancel={launcher.cancel}
                />
              ) : null}
            </div>
          </div>
        </div>

        {/* 登录后的本地持久化提示：放在账号区之外，未登录/已登录都可达。 */}
        {accountNotice ? (
          <InlineNotice tone="warning" icon={<IconAlert />} role="status">
            {accountNotice}
          </InlineNotice>
        ) : null}

        {/* 启动进度顶替角色区，底部操作保持原有占位。 */}
        <div className="home__launch home__stage home__stage--3">
          <>
            <LaunchButton
              running={launcher.running}
              disabled={!canLaunch}
              onClick={handleLaunch}
              label={
                launcher.starting
                  ? launcher.cancelRequested
                    ? "正在取消..."
                    : "正在启动..."
                  : launcher.running
                  ? "游戏运行中"
                  : "启动游戏"
              }
              subLabel={
                instance
                  ? instance.displayName || instance.versionId
                  : "未选择实例"
              }
            />

            <div className="home__launch-secondary">
              <Button
                variant="ghost"
                className="home__ghost"
                iconLeft={<IconLayers />}
                disabled={launcher.starting}
                onClick={() => setRoute("instances")}
              >
                选择版本
              </Button>
              <Button
                variant="ghost"
                className="home__ghost"
                iconLeft={<IconSliders />}
                disabled={!instance || launcher.starting}
                onClick={() => instance && openVersionSettings(instance)}
              >
                版本设置
              </Button>
            </div>
          </>

          {launchError ? (
            <LaunchErrorDialog
              key={`${reportTarget?.gameDir ?? ""}:${
                reportTarget?.versionId ?? ""
              }:${launchError.reason}`}
              open
              title={launchError.title}
              reason={launchError.reason}
              message={launchError.message}
              onClose={launcher.clearError}
              onOpenLogs={reportTarget ? handleOpenLogs : undefined}
              onExportReport={reportTarget ? handleExportReport : undefined}
            />
          ) : null}
        </div>
      </Card>

      {/* 右侧整片留白：不渲染任何元素，让背景图直接透出 */}
      <div className="home__blank" aria-hidden="true" />

      {/* 切换账号：已保存的账号互切，或退出当前账号去登录另一个 */}
      <AccountSwitcherModal
        open={switcherOpen}
        onClose={() => setSwitcherOpen(false)}
        accounts={accounts}
        activeId={activeId}
        onSelect={handleSwitchAccount}
        onRemove={(id) => void remove(id)}
        onLoginOther={handleLoginOther}
      />
      {/* 皮肤站角色列表：缓存即时渲染，选中后静默切换角色 */}
      <CharacterPickerModal
        open={pickerOpen}
        account={accountType === "littleskin" ? typedAccount : null}
        onClose={() => setPickerOpen(false)}
        onSelect={handleSwitchProfile}
        onOpenSite={() => void handleOpenSkinSite()}
      />

      {/* 皮肤站登录弹窗：仅在没有可用会话时出现 */}
      <SkinLoginDialog
        open={skinDialogOpen}
        server={settings.littleskinServer ?? ""}
        onClose={() => setSkinDialogOpen(false)}
        onAccount={applySkinAccount}
        onProfileSelect={handleProfileSelect}
      />

      {/* 多角色本地切换：会话仍在时无需重新输入密码 */}
      {profileSwitch ? (
        <SkinProfileDialog
          open
          sessionId={profileSwitch.sessionId}
          profiles={profileSwitch.profiles}
          server={settings.littleskinServer ?? ""}
          onResolved={(account) => {
            void applySkinAccount(account);
            setProfileSwitch(null);
          }}
          onCancelled={() => {
            setProfileSwitch(null);
            setSkinSession(null);
          }}
        />
      ) : null}
    </div>
  );
}

export default Home;
