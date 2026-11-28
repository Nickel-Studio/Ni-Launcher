import { useSkinAuthContext, sameSkinSite } from "../lib/skinAuth";
import { SkinOAuthLogin } from "../components/account/SkinOAuthLogin";
import { SkinSiteIcon } from "../components/account/SkinSiteIcon";

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useSettings } from "../lib/settings";
import { showToast } from "../lib/toast";
import { openVersionSettings, setRoute } from "../lib/nav";
import { useLaunchInstance } from "../lib/instances";
import {
  Button,
  Card,
  IconButton,
  IconChevronDown,
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
import { RetainedLaunchContent } from "../components/launch/LaunchTransition";
import "./Home.css";

const AVATAR_SIZE = 72;

const EMPTY_IDENTITY: Identity = { name: "", email: "", loggedIn: false };
// Preserve ordinary-mode selection when navigating through exclusive instances.
let ordinaryAccountId: string | null = null;

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
    if (key === display) {
      setPhase("in");
      return;
    }
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
  const { context: skinContext, ready: skinContextReady } =
    useSkinAuthContext();
  const [loginOther, setLoginOther] = useState<AccountType | null>(null);
  const [savingAccount, setSavingAccount] = useState(false);
  const accountType = skinContext.exclusive
    ? "littleskin"
    : loginOther ?? settings.accountType;
  const skinServer = skinContext.site.server;

  // ---- 多账号 ----
  const {
    accounts,
    activeId,
    loading: accountsLoading,
    upsert,
    remove,
    setActive,
  } = useAccounts();

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
  useEffect(() => {
    setSkinDialogOpen(false);
    setProfileSwitch(null);
    setPickerOpen(false);
    setSkinSession(null);
  }, [skinServer, skinContext.root, skinContext.versionId, skinContext.method]);

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
      loginOther === null &&
      activeAccount &&
      activeAccount.type === accountType &&
      (accountType !== "littleskin" ||
        sameSkinSite(activeAccount.server ?? "", skinServer))
        ? activeAccount
        : null,
    [activeAccount, accountType, skinServer, loginOther]
  );

  const reconciledScope = useRef("");
  const accountScope = JSON.stringify([
    skinContext.exclusive,
    accountType,
    skinServer,
    skinContext.root,
    skinContext.versionId,
  ]);
  const authScope = useRef(accountScope);
  authScope.current = accountScope;
  useEffect(() => {
    if (
      !skinContextReady ||
      loginOther !== null ||
      accountsLoading ||
      reconciledScope.current === accountScope
    )
      return;
    reconciledScope.current = accountScope;
    if (
      skinContext.exclusive &&
      activeAccount &&
      !typedAccount &&
      ordinaryAccountId === null
    )
      ordinaryAccountId = activeId;
    const matches = (account: Account) =>
      account.type === accountType &&
      (accountType !== "littleskin" ||
        sameSkinSite(account.server ?? "", skinServer));
    const candidate =
      accountType === "offline"
        ? ""
        : typedAccount?.id ??
          accounts.find(
            (account) => account.id === ordinaryAccountId && matches(account)
          )?.id ??
          accounts.find(matches)?.id ??
          "";
    if (candidate !== activeId) {
      setAccountSwitching(true);
      void setActive(candidate)
        .then((ok) => {
          if (!ok) showToast("账号切换未能保存，请重试。", "error");
        })
        .finally(() => setAccountSwitching(false));
    }
  }, [
    accountScope,
    loginOther,
    skinContextReady,
    accountsLoading,
    accounts,
    activeId,
    activeAccount,
    typedAccount,
    accountType,
    skinServer,
    skinContext.exclusive,
    setActive,
  ]);
  useEffect(() => {
    if (skinContextReady && !skinContext.exclusive && typedAccount)
      ordinaryAccountId = typedAccount.id;
  }, [skinContextReady, skinContext.exclusive, typedAccount]);

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
      if (skinContext.exclusive) return;
      const next = value as AccountType;
      if (loginOther !== null) {
        setLoginOther(next);
        return;
      }
      update({ accountType: next });
      // 离线形态没有后端账号，清掉 active 避免继续沿用上一次的在线身份。
      const nextActive =
        next === "offline"
          ? ""
          : accounts.find(
              (a) =>
                a.type === next &&
                (next !== "littleskin" ||
                  sameSkinSite(a.server ?? "", skinServer))
            )?.id ?? "";
      // 激活命令完成前置为切换中，启动按钮暂时不可用。
      setAccountSwitching(true);
      void setActive(nextActive).finally(() => setAccountSwitching(false));
    },
    [update, accounts, setActive, skinContext.exclusive, skinServer, loginOther]
  );

  /** Authentication is already complete; Home alone notifies the final persistence result. */
  const applyMicrosoftAccount = useCallback(
    async (account: Account) => {
      setSavingAccount(true);
      const result = await persistAccount({
        id: account.id,
        type: "microsoft",
        name: account.name || "Player",
        uuid: account.uuid,
        email: account.email,
        server: undefined,
      });
      setSavingAccount(false);
      setLoginOther(null);
      update({ accountType: "microsoft" });
      setAvatarNonce((n) => n + 1);
      const notice = persistNotice(result, MICROSOFT_PERSIST_MESSAGES);
      showToast(notice || "Microsoft 登录成功", notice ? "error" : "success");
    },
    [persistAccount, update]
  );

  const applySkinAccount = useCallback(
    async (account: SkinAccount) => {
      setSavingAccount(true);
      const result = await persistAccount({
        id: account.id,
        type: "littleskin",
        name: account.name || "Player",
        uuid: account.uuid ?? undefined,
        email: account.email ?? undefined,
        server: account.server ?? skinServer,
        authMethod: account.authMethod,
      });
      setSavingAccount(false);
      setLoginOther(null);
      if (!skinContext.exclusive) update({ accountType: "littleskin" });
      const session = getSkinSession();
      const profiles: SkinProfile[] =
        session?.profiles ??
        (account.uuid
          ? [{ id: account.uuid.replace(/-/g, ""), name: account.name }]
          : []);
      writeCachedProfiles(account.id, profiles);
      setAvatarNonce((n) => n + 1);
      const notice = persistNotice(result, SKIN_PERSIST_MESSAGES);
      showToast(
        notice || `${skinContext.site.name} 登录成功`,
        notice ? "error" : "success"
      );
    },
    [
      persistAccount,
      skinServer,
      skinContext.site.name,
      skinContext.exclusive,
      update,
    ]
  );

  /** 皮肤站返回多角色：统一在首页弹出单选弹窗（若登录发生在弹窗里，先关掉那个弹窗）。 */
  const handleProfileSelect = useCallback((session: SkinSession) => {
    setSkinDialogOpen(false);
    setProfileSwitch(session);
  }, []);

  const handleLogout = useCallback(() => {
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
      if (
        !target ||
        (skinContext.exclusive &&
          (target.type !== "littleskin" ||
            !sameSkinSite(target.server ?? "", skinServer)))
      )
        return;
      if (!skinContext.exclusive && target.type !== accountType)
        update({ accountType: target.type });
      // 激活命令完成前置为切换中，避免「切账号后立刻启动」沿用旧账号身份。
      setAccountSwitching(true);
      void setActive(id).finally(() => setAccountSwitching(false));
      setSkinSession(null);
      setAvatarNonce((n) => n + 1);
    },
    [
      accounts,
      activeId,
      accountType,
      update,
      setActive,
      skinContext.exclusive,
      skinServer,
    ]
  );

  // A temporary login form must not log out or overwrite the original account.
  const handleLoginOther = useCallback(() => {
    setSwitcherOpen(false);
    setLoginOther(accountType);
  }, [accountType]);

  const handleCancelLoginOther = useCallback(() => {
    if (savingAccount) return;
    setLoginOther(null);
    setSkinDialogOpen(false);
    setProfileSwitch(null);
    setSkinSession(null);
  }, [savingAccount]);

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
    const raw = (typedAccount?.server ?? skinServer ?? "").trim();
    const site = raw.replace(/\/+$/, "") || "https://littleskin.cn";
    const root = /^https?:\/\//i.test(site) ? site : `https://${site}`;
    return `${stripYggdrasilRoot(root)}/user/player`;
  }, [typedAccount?.server, skinServer]);

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
      const current = authScope.current;
      const updated = await invoke<SkinAccount>("yggdrasil_select_profile", {
        sessionId,
        profileId: profile.id,
      });
      if (
        current !== authScope.current ||
        !sameSkinSite(updated.server ?? "", skinServer)
      )
        return;
      const result = await persistAccount({
        id: updated.id,
        type: "littleskin",
        name: updated.name || profile.name,
        uuid: updated.uuid ?? undefined,
        email: updated.email ?? typedAccount?.email ?? undefined,
        server: updated.server ?? typedAccount?.server ?? undefined,
        authMethod: updated.authMethod ?? typedAccount?.authMethod,
      });
      const notice = persistNotice(result, SKIN_PERSIST_MESSAGES);
      if (notice) showToast(notice, "error");
      setAvatarNonce((n) => n + 1);
    },
    [typedAccount, persistAccount, skinServer]
  );

  const canLaunch =
    skinContextReady &&
    loginOther === null &&
    instance !== null &&
    launchUsername.trim() !== "" &&
    (accountType === "offline" || typedAccount !== null) &&
    !launcher.starting &&
    !savingAccount &&
    !accountSwitching;

  /** 启动失败只在弹窗里出现一次；诊断优先于普通错误。 */
  const launchError = launchErrorInfo(launcher.error, launcher.diagnostic);

  const handleLaunch = useCallback(() => {
    if (!canLaunch || !instance) return;
    void launcher.launch({
      versionId: instance.versionId,
      gameDir: instance.root,
      displayName: instance.displayName || instance.versionId,
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

  const accountForm =
    accountType === "littleskin" && skinContext.method === "oauth2" ? (
      <SkinOAuthLogin
        context={skinContext}
        onAccount={applySkinAccount}
        onProfileSelect={handleProfileSelect}
        disabled={!skinContextReady}
      />
    ) : (
      <AccountPanel
        accountType={accountType}
        identity={effectiveIdentity}
        littleskinServer={skinServer ?? ""}
        onMicrosoftAccount={applyMicrosoftAccount}
        onSkinAccount={applySkinAccount}
        onLogout={handleLogout}
        onProfileSelect={handleProfileSelect}
      />
    );

  // 角色区 / 表单 —— 账号形态或登录态变化时淡出→淡入。
  const swapKey = `${accountType}:${
    loginOther !== null ? "adding" : effectiveIdentity.loggedIn ? "in" : "out"
  }`;
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
          server={typedAccount?.server ?? skinServer}
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
          {!isOffline && typedAccount?.authMethod !== "oauth2" ? (
            <p className="home__email">{emailText}</p>
          ) : null}
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
          inert={launcher.starting || savingAccount}
        >
          {skinContext.exclusive ? (
            <Tooltip label={skinServer}>
              <div className="home__skin-title">
                <SkinSiteIcon site={skinContext.site} />
                {skinContext.site.name} 登录
              </div>
            </Tooltip>
          ) : (
            <AccountTypeTabs
              className="home__accounts"
              value={accountType}
              onChange={handleAccountTypeChange}
            />
          )}
        </div>
        {loginOther !== null ? (
          <Button
            className="home__login-back"
            variant="ghost"
            size="sm"
            iconLeft={<IconChevronDown className="home__back-icon" />}
            disabled={savingAccount}
            onClick={handleCancelLoginOther}
          >
            返回
          </Button>
        ) : null}

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
              <RetainedLaunchContent visible={launcher.starting}>
                <LaunchPanel
                  instanceName={
                    launcher.activeTarget?.displayName ||
                    launcher.activeTarget?.versionId ||
                    "未选择实例"
                  }
                  phase={launcher.phase}
                  stage={launcher.stage}
                  progress={launcher.progress}
                  percent={launcher.percent}
                  speedBps={launcher.speedBps}
                  cancelRequested={launcher.cancelRequested}
                />
              </RetainedLaunchContent>
            </div>
          </div>
        </div>

        {/* 启动进度顶替角色区，底部操作保持原有占位。 */}
        <div className="home__launch home__stage home__stage--3">
          <>
            <LaunchButton
              starting={launcher.starting}
              cancelRequested={launcher.cancelRequested}
              disabled={!canLaunch}
              onClick={handleLaunch}
              onCancel={launcher.cancel}
              subLabel={
                launcher.starting
                  ? launcher.activeTarget?.displayName ||
                    launcher.activeTarget?.versionId
                  : instance?.displayName || instance?.versionId || "未选择实例"
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

          <LaunchErrorDialog
            reportKey={launcher.failureId}
            open={Boolean(launchError)}
            title={launchError?.title ?? "启动失败"}
            reason={launchError?.reason ?? ""}
            message={launchError?.message ?? ""}
            onClose={launcher.clearError}
            onOpenLogs={reportTarget ? handleOpenLogs : undefined}
            onExportReport={reportTarget ? handleExportReport : undefined}
          />
        </div>
      </Card>

      {/* 右侧整片留白：不渲染任何元素，让背景图直接透出 */}
      <div className="home__blank" aria-hidden="true" />

      {/* 切换账号：已保存账号互切，或保留当前账号临时登录另一个 */}
      <AccountSwitcherModal
        open={switcherOpen}
        onClose={() => setSwitcherOpen(false)}
        accounts={
          skinContext.exclusive
            ? accounts.filter(
                (account) =>
                  account.type === "littleskin" &&
                  sameSkinSite(account.server ?? "", skinServer)
              )
            : accounts
        }
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
        server={skinServer ?? ""}
        onClose={() => setSkinDialogOpen(false)}
        onAccount={applySkinAccount}
        onProfileSelect={handleProfileSelect}
      />

      {/* 多角色本地切换：会话仍在时无需重新输入密码 */}
      <SkinProfileDialog
        open={profileSwitch !== null}
        sessionId={profileSwitch?.sessionId ?? ""}
        profiles={profileSwitch?.profiles ?? []}
        server={skinServer ?? ""}
        oauthSiteName={profileSwitch?.oauthSiteName}
        onResolved={async (account) => {
          await applySkinAccount(account);
          setProfileSwitch(null);
        }}
        onCancelled={() => {
          setProfileSwitch(null);
          setSkinSession(null);
        }}
      />
    </div>
  );
}

export default Home;
