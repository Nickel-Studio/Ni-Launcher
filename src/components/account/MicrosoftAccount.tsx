import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button, Modal } from "../ui";
import { isTauri } from "../../lib/settings";
import { showToast } from "../../lib/toast";
import { notifyLoginError } from "./loginFeedback";
import type { Account, Identity } from "./types";
import "./account.css";

export interface MicrosoftAccountProps {
  identity: Identity;
  /**
   * 设备码成功后交回后端已落库的公开账号（含 id / uuid）。
   * 入库与激活统一由调用方完成，组件不再重复调用 accounts_* 命令。
   */
  onAccount: (account: Account) => void | Promise<void>;
  onLogout: () => void;
}

/** `msa_config_get` 返回的配置（后端序列化为 camelCase）。 */
interface MsaConfig {
  clientId: string;
  authority: string;
  redirectUri: string;
}

/** `msa_device_start` 返回的设备码信息。 */
interface DeviceCodeStart {
  userCode: string;
  deviceCode: string;
  verificationUri: string;
  interval: number;
  expiresIn: number;
}

/** `msa_device_poll` 返回的轮询结果。 */
interface PollResult {
  status: "pending" | "success";
  account?: Account | null;
}

type Phase = "idle" | "starting" | "waiting" | "saving";

const NO_CONFIG_HINT =
  "未检测到 secrets.local.json，请复制 secrets.local.example.json 并填入 Microsoft Entra 的 Client ID";

/**
 * 正版（Microsoft）账号形态：真实的 OAuth 2.0 Device Code Flow。
 *
 * 表单本身只保留一个居中的「使用 Microsoft 登录」按钮；点击后弹出设备码弹窗
 * （PCL 风格：强调色标题 + 设备码大字 + 校验链接 + 重新打开网页 / 复制代码 / 取消），
 * 不再就地替换表单内容。
 *
 * 轮询按 `interval` 秒进行，到 `expiresIn` 停止；关闭弹窗（取消）立即停止轮询，
 * 组件卸载同样清理定时器，绝不留下悬挂的计时器。
 */
export function MicrosoftAccount({
  identity,
  onAccount,
  onLogout,
}: MicrosoftAccountProps) {
  const [configReady, setConfigReady] = useState(false);
  const [config, setConfig] = useState<MsaConfig | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [device, setDevice] = useState<DeviceCodeStart | null>(null);
  const [copied, setCopied] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);

  const timerRef = useRef<number | null>(null);
  const mountedRef = useRef(true);
  const pendingRef = useRef(false);
  const savingRef = useRef(false);
  const flowRef = useRef(0);

  const stopPolling = useCallback(() => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    let alive = true;
    if (!isTauri()) {
      setConfigReady(true);
    } else {
      void invoke<MsaConfig | null>("msa_config_get")
        .then((cfg) => {
          if (alive) setConfig(cfg ?? null);
        })
        .catch(() => {
          if (alive) setConfig(null);
        })
        .finally(() => {
          if (alive) setConfigReady(true);
        });
    }
    return () => {
      alive = false;
      mountedRef.current = false;
      flowRef.current++;
      pendingRef.current = false;
      stopPolling();
    };
  }, [stopPolling]);

  const endFlow = useCallback(() => {
    // Invalidate in-flight replies before closing, so a late rejection cannot notify twice.
    flowRef.current++;
    pendingRef.current = false;
    savingRef.current = false;
    stopPolling();
    setDevice(null);
    setPhase("idle");
    setDialogOpen(false);
  }, [stopPolling]);

  const failLogin = useCallback(
    (error: unknown) => {
      endFlow();
      notifyLoginError("Microsoft", error);
    },
    [endFlow]
  );

  const startPolling = useCallback(
    (
      clientId: string,
      deviceCode: string,
      intervalSec: number,
      deadline: number,
      flowId: number
    ) => {
      const gap = Math.max(1, intervalSec || 5) * 1000;
      const isCurrent = () => mountedRef.current && flowId === flowRef.current;
      const tick = async () => {
        if (!isCurrent()) return;
        if (Date.now() >= deadline) {
          failLogin("设备代码已过期，请重新开始登录");
          return;
        }
        try {
          const res = await invoke<PollResult>("msa_device_poll", {
            clientId,
            deviceCode,
          });
          if (!isCurrent()) return;
          if (res.status === "success") {
            if (!res.account) throw new Error("未获取到有效账号，请重新登录");
            stopPolling();
            savingRef.current = true;
            setPhase("saving");
            // Home owns the final success/persistence notification, including after unmount.
            try {
              await onAccount(res.account);
            } catch {
              if (isCurrent()) {
                showToast(
                  "Microsoft 账号已认证，但保存到本地配置失败，请重试。",
                  "error"
                );
              }
            }
            if (isCurrent()) endFlow();
          } else {
            timerRef.current = window.setTimeout(tick, gap);
          }
        } catch (err) {
          if (isCurrent()) failLogin(err);
        }
      };
      timerRef.current = window.setTimeout(tick, gap);
    },
    [onAccount, stopPolling, endFlow, failLogin]
  );

  const startLogin = useCallback(async () => {
    if (!config || pendingRef.current) return;
    pendingRef.current = true;
    savingRef.current = false;
    const flowId = ++flowRef.current;
    stopPolling();
    setCopied(false);
    setDevice(null);
    setPhase("starting");
    setDialogOpen(true);
    try {
      const dc = await invoke<DeviceCodeStart>("msa_device_start", {
        clientId: config.clientId,
      });
      if (!mountedRef.current || flowId !== flowRef.current) return;
      setDevice(dc);
      setPhase("waiting");
      const ttl = (dc.expiresIn > 0 ? dc.expiresIn : 900) * 1000;
      startPolling(
        config.clientId,
        dc.deviceCode,
        dc.interval,
        Date.now() + ttl,
        flowId
      );
    } catch (err) {
      if (mountedRef.current && flowId === flowRef.current) failLogin(err);
    }
  }, [config, startPolling, stopPolling, failLogin]);

  const closeDialog = useCallback(() => {
    if (!pendingRef.current || savingRef.current) return;
    endFlow();
    showToast("Microsoft 登录已取消", "info");
  }, [endFlow]);

  const copyCode = useCallback(async () => {
    if (!device) return;
    const flowId = flowRef.current;
    try {
      await navigator.clipboard.writeText(device.userCode);
      if (!mountedRef.current || flowId !== flowRef.current) return;
      setCopied(true);
      window.setTimeout(() => {
        if (mountedRef.current && flowId === flowRef.current) setCopied(false);
      }, 2000);
    } catch {
      if (mountedRef.current && flowId === flowRef.current) {
        showToast("无法复制设备代码，请手动输入。", "error");
      }
    }
  }, [device]);

  const openVerification = useCallback(async () => {
    if (!device) return;
    const flowId = flowRef.current;
    try {
      await openUrl(device.verificationUri);
    } catch {
      if (mountedRef.current && flowId === flowRef.current) {
        showToast(
          `无法自动打开浏览器，请手动访问 ${device.verificationUri}`,
          "error"
        );
      }
    }
  }, [device]);

  if (identity.loggedIn) {
    return (
      <div className="account-form">
        <p className="account-form__status">
          <span className="account-form__ok" aria-hidden="true" />
          已通过 Microsoft 账号登录
        </p>
        <div className="account-form__row">
          <Button variant="ghost" onClick={onLogout}>
            退出登录
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="account-form">
      {!configReady ? (
        <p className="account-form__note">正在检测本地登录配置...</p>
      ) : null}
      {configReady && !config ? (
        <p className="account-form__hint" role="status">
          {NO_CONFIG_HINT}
        </p>
      ) : null}
      <div className="account-form__row account-form__row--center">
        <Button
          variant="primary"
          icon="microsoft"
          disabled={!configReady || !config || dialogOpen}
          onClick={() => void startLogin()}
        >
          使用 Microsoft 登录
        </Button>
      </div>
      <Modal
        open={dialogOpen}
        onClose={closeDialog}
        dismissible={phase !== "saving"}
        className="account-dialog"
        title="登录 Minecraft"
        width={440}
        footer={
          <>
            <Button
              variant="ghost"
              disabled={!device || phase === "saving"}
              onClick={() => void openVerification()}
            >
              重新打开网页
            </Button>
            <Button
              variant="ghost"
              disabled={!device || phase === "saving"}
              onClick={() => void copyCode()}
            >
              {copied ? "已复制" : "复制代码"}
            </Button>
            <Button
              variant="secondary"
              disabled={phase === "saving"}
              onClick={closeDialog}
            >
              取消
            </Button>
          </>
        }
      >
        <div className="account-dialog__body">
          {phase === "starting" ? (
            <p className="account-dialog__status">正在获取设备码...</p>
          ) : null}
          {phase === "saving" ? (
            <p className="account-dialog__status">正在保存账号…</p>
          ) : null}
          {device && phase !== "saving" ? (
            <>
              <p className="account-dialog__lead">
                在浏览器中输入以下代码完成登录
              </p>
              <div className="account-dialog__code-row">
                <button
                  type="button"
                  className="account-dialog__code"
                  onClick={() => void copyCode()}
                >
                  {device.userCode}
                </button>
              </div>
              <button
                type="button"
                className="account-dialog__link"
                onClick={() => void openVerification()}
              >
                {device.verificationUri}
              </button>
            </>
          ) : null}
        </div>
      </Modal>
    </div>
  );
}

export default MicrosoftAccount;
