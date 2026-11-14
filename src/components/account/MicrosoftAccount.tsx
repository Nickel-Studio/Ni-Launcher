import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button, Modal } from "../ui";
import { isTauri } from "../../lib/settings";
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

type Phase = "idle" | "starting" | "waiting" | "saving" | "error";

/** 把后端 / 运行时抛出的值统一成可读文案（后端已把 XSTS 错误码转成中文）。 */
function toReadableError(err: unknown): string {
  if (typeof err === "string" && err.trim()) return err;
  if (err instanceof Error && err.message) return err.message;
  return "登录失败，请稍后重试";
}

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
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);

  const timerRef = useRef<number | null>(null);
  const cancelledRef = useRef(false);
  const mountedRef = useRef(true);
  /** 设备码请求进行中：同步拦截重复点击，避免并发出第二条设备码流程。 */
  const startingRef = useRef(false);
  /** 当前设备码流程编号：取消 / 重新登录后，旧流程的在途结果一律丢弃。 */
  const flowRef = useRef(0);

  const stopPolling = useCallback(() => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  // 挂载时读取密钥配置；非 Tauri 环境直接降级为「未配置」。
  useEffect(() => {
    mountedRef.current = true;
    if (!isTauri()) {
      setConfigReady(true);
      return () => {
        mountedRef.current = false;
      };
    }
    void (async () => {
      try {
        const cfg = await invoke<MsaConfig | null>("msa_config_get");
        if (mountedRef.current) setConfig(cfg ?? null);
      } catch {
        if (mountedRef.current) setConfig(null);
      } finally {
        if (mountedRef.current) setConfigReady(true);
      }
    })();
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // 卸载时取消轮询，避免定时器泄漏与卸载后 setState。
  useEffect(
    () => () => {
      cancelledRef.current = true;
      stopPolling();
    },
    [stopPolling]
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

      const tick = async () => {
        if (
          cancelledRef.current ||
          !mountedRef.current ||
          flowId !== flowRef.current
        ) {
          return;
        }
        if (Date.now() >= deadline) {
          stopPolling();
          setPhase("error");
          setError("设备代码已过期，请重新开始登录");
          return;
        }
        try {
          const res = await invoke<PollResult>("msa_device_poll", {
            clientId,
            deviceCode,
          });
          if (
            cancelledRef.current ||
            !mountedRef.current ||
            flowId !== flowRef.current
          ) {
            return;
          }

          if (res.status === "success" && res.account) {
            const account = res.account;
            stopPolling();
            // 登录已成功，但还要把账号同步进本地列表。保存期间保持弹窗打开：
            // 若保存意外抛错，错误提示才处于用户可见的状态；保存完成后再关闭。
            setPhase("saving");
            setError("");
            try {
              // 后端 msa_device_poll 已完成落库；入库与激活统一交给调用方，
              // 避免组件与首页各写一次 accounts_* 造成重复记录。
              await onAccount(account);
            } catch {
              if (!mountedRef.current || flowId !== flowRef.current) return;
              setPhase("error");
              setError("账号已登录，但保存到本地配置失败，请关闭窗口后重试。");
              return;
            }
            if (!mountedRef.current || flowId !== flowRef.current) return;
            setDevice(null);
            setPhase("idle");
            setDialogOpen(false);
          } else {
            timerRef.current = window.setTimeout(tick, gap);
          }
        } catch (err) {
          if (
            cancelledRef.current ||
            !mountedRef.current ||
            flowId !== flowRef.current
          ) {
            return;
          }
          stopPolling();
          setPhase("error");
          setError(toReadableError(err));
        }
      };

      timerRef.current = window.setTimeout(tick, gap);
    },
    [onAccount, stopPolling]
  );

  const startLogin = useCallback(async () => {
    if (!config || startingRef.current) return;
    // 同步占位：同一帧内的重复点击不会发出第二条设备码请求。
    startingRef.current = true;
    const flowId = ++flowRef.current;
    cancelledRef.current = false;
    stopPolling();
    setError("");
    setCopied(false);
    setDevice(null);
    setPhase("starting");
    setDialogOpen(true);
    try {
      const dc = await invoke<DeviceCodeStart>("msa_device_start", {
        clientId: config.clientId,
      });
      if (
        !mountedRef.current ||
        cancelledRef.current ||
        flowId !== flowRef.current
      ) {
        return;
      }
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
      if (
        !mountedRef.current ||
        cancelledRef.current ||
        flowId !== flowRef.current
      ) {
        return;
      }
      setPhase("error");
      setError(toReadableError(err));
    } finally {
      // 只由当前流程清空占位：旧流程的收尾不能解除新流程的防重复保护。
      if (flowId === flowRef.current) startingRef.current = false;
    }
  }, [config, startPolling, stopPolling]);

  /** 取消 = 关闭弹窗并停止轮询。 */
  const closeDialog = useCallback(() => {
    cancelledRef.current = true;
    // 取消时让当前流程编号失效：在途的设备码 / 轮询 / 保存结果不再回写状态。
    flowRef.current += 1;
    startingRef.current = false;
    stopPolling();
    setDevice(null);
    setPhase("idle");
    setError("");
    setDialogOpen(false);
  }, [stopPolling]);

  const copyCode = useCallback(async () => {
    if (!device) return;
    try {
      await navigator.clipboard.writeText(device.userCode);
      setCopied(true);
      window.setTimeout(() => {
        if (mountedRef.current) setCopied(false);
      }, 2000);
    } catch {
      setCopied(false);
    }
  }, [device]);

  const openVerification = useCallback(async () => {
    if (!device) return;
    try {
      await openUrl(device.verificationUri);
    } catch {
      setError(`无法自动打开浏览器，请手动访问 ${device.verificationUri}`);
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

      {error && !dialogOpen ? (
        <p className="account-form__error" role="alert">
          {error}
        </p>
      ) : null}

      <div className="account-form__row account-form__row--center">
        <Button
          variant="primary"
          icon="microsoft"
          disabled={
            !configReady ||
            !config ||
            dialogOpen ||
            phase === "starting" ||
            phase === "saving"
          }
          onClick={() => void startLogin()}
        >
          使用 Microsoft 登录
        </Button>
      </div>

      {dialogOpen ? (
        <Modal
          open
          onClose={closeDialog}
          className="account-dialog"
          title="登录 Minecraft"
          width={440}
          footer={
            <>
              <Button
                variant="ghost"
                disabled={!device}
                onClick={() => void openVerification()}
              >
                重新打开网页
              </Button>
              <Button
                variant="ghost"
                disabled={!device}
                onClick={() => void copyCode()}
              >
                {copied ? "已复制" : "复制代码"}
              </Button>
              <Button variant="secondary" onClick={closeDialog}>
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

            {error ? (
              <p className="account-form__error" role="alert">
                {error}
              </p>
            ) : null}
          </div>
        </Modal>
      ) : null}
    </div>
  );
}

export default MicrosoftAccount;
