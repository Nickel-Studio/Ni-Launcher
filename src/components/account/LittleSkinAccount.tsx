import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AnimatedSwap, Button } from "../ui";
import { isTauri } from "../../lib/settings";
import { IconEye, IconEyeOff, IconLock, IconUser } from "./icons";
import {
  setSkinSession,
  toReadableError,
  type SkinAccount,
  type SkinLoginOutcome,
  type SkinSession,
} from "./yggdrasil";
import type { Identity } from "./types";
import "./account.css";

export interface LittleSkinAccountProps {
  identity: Identity;
  /** 皮肤站地址，来自设置页的 littleskinServer（登录界面不再提供输入框）。 */
  server: string;
  /**
   * 登录成功后交回后端落库的**权威账号**（带 id / uuid / 角色名）。
   * 不要再由前端拼一个同名账号 —— 那样会多出一条没有 uuid 与令牌的影子记录。
   */
  onAccount: (account: SkinAccount) => void | Promise<void>;
  onLogout?: () => void;
  /**
   * status="select" 时上报多角色会话：由调用方（首页）统一弹出单选弹窗。
   * 这样无论登录发生在内联表单还是弹窗里，选角弹窗都只有一处、不会出现弹窗套弹窗。
   */
  onProfileSelect: (session: SkinSession) => void;
}

type Phase = "identify" | "password";

/**
 * 皮肤站（LittleSkin）账号形态：真实的 Yggdrasil 认证，两阶段输入。
 *
 * 阶段一：占位符用户图标 + 邮箱 +（紧贴输入框右侧的）「下一步」；
 * 阶段二：只读邮箱回显 + 密码 +「登录」（支持 Enter 提交）。
 * 表单里没有任何说明性 / 免责性文案 —— 需要用户决策的输入才出现在界面上。
 *
 * 密码只在阶段二到 `yggdrasil_login` 返回值之间存活，成功后立即从内存状态丢弃，
 * 不写入任何持久化存储（后端同样只保留会话令牌）。
 *
 * `yggdrasil_login` 返回值是包了一层的 LoginOutcome：
 *   - status="account" → 直接拿到账号；
 *   - status="select"  → 交给首页弹出单选弹窗，再调 `yggdrasil_select_profile`。
 */
export function LittleSkinAccount({
  identity,
  server,
  onAccount,
  onLogout,
  onProfileSelect,
}: LittleSkinAccountProps) {
  const [phase, setPhase] = useState<Phase>("identify");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const emailInput = useRef<HTMLInputElement>(null);
  const passwordInput = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  /** 收尾：把后端落库的权威账号交给调用方（入库与激活都在那边完成）。 */
  const finish = useCallback(
    async (account: SkinAccount) => {
      await onAccount(account);
    },
    [onAccount]
  );

  const submit = useCallback(async () => {
    const username = email.trim();
    const endpoint = server.trim();
    if (!username || !password || !endpoint || busy) return;

    if (!isTauri()) {
      setError("当前环境不支持登录（请在桌面应用中运行）");
      return;
    }

    setBusy(true);
    setError("");
    try {
      const outcome = await invoke<SkinLoginOutcome>("yggdrasil_login", {
        server: endpoint,
        username,
        password,
      });

      // 密码使命结束：立即从内存状态丢弃，不做任何持久化。
      setPassword("");

      if (outcome.status === "select") {
        if (outcome.profiles.length === 0) {
          setError("该账号下没有可用角色，请先到皮肤站创建角色");
          return;
        }
        setSkinSession({
          sessionId: outcome.sessionId,
          profiles: outcome.profiles,
        });
        onProfileSelect({
          sessionId: outcome.sessionId,
          profiles: outcome.profiles,
        });
        return;
      }

      setSkinSession(null);
      await finish(outcome.account);
    } catch (err) {
      setError(toReadableError(err));
    } finally {
      setBusy(false);
    }
  }, [busy, email, finish, onProfileSelect, password, server]);

  const next = useCallback(() => {
    if (!email.trim()) return;
    setError("");
    setPhase("password");
  }, [email]);

  const back = useCallback(() => {
    setPhase("identify");
    setPassword("");
    setShowPassword(false);
    setError("");
  }, []);

  // 两阶段动画会同时挂载旧层快照；不用 autoFocus，避免旧输入框抢焦点。
  useEffect(() => {
    if (identity.loggedIn) return;
    const target =
      phase === "identify" ? emailInput.current : passwordInput.current;
    target?.focus();
  }, [phase, identity.loggedIn]);

  if (identity.loggedIn) {
    return (
      <div className="account-form">
        <p className="account-form__status">
          <span className="account-form__ok" aria-hidden="true" />
          已通过 LittleSkin 登录 · {server}
        </p>
        {onLogout ? (
          <div className="account-form__row">
            <Button variant="ghost" onClick={onLogout}>
              退出登录
            </Button>
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <form
      className="skin-login"
      onSubmit={(e) => {
        e.preventDefault();
        if (phase === "identify") next();
        else void submit();
      }}
    >
      <AnimatedSwap swapKey={phase} className="skin-login__stage">
        {phase === "identify" ? (
          <div className="skin-login__field">
            <span className="skin-login__icon" aria-hidden="true">
              <IconUser />
            </span>
            <input
              ref={emailInput}
              className="skin-login__input"
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="邮箱"
              autoComplete="username"
              spellCheck={false}
            />
            <button
              type="submit"
              className="skin-login__submit"
              disabled={!email.trim() || busy}
            >
              下一步
            </button>
          </div>
        ) : (
          <>
            <div className="skin-login__who-row">
              <span className="skin-login__who">{email.trim()}</span>
              <button type="button" className="skin-login__back" onClick={back}>
                返回
              </button>
            </div>
            <div className="skin-login__field">
              <span className="skin-login__icon" aria-hidden="true">
                <IconLock />
              </span>
              <input
                ref={passwordInput}
                className="skin-login__input"
                type={showPassword ? "text" : "password"}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="密码"
                autoComplete="current-password"
              />
              <button
                type="button"
                className="skin-login__reveal"
                aria-label={showPassword ? "隐藏密码" : "显示密码"}
                aria-pressed={showPassword}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => {
                  const input = passwordInput.current;
                  const start = input?.selectionStart ?? null;
                  const end = input?.selectionEnd ?? null;
                  setShowPassword((visible) => !visible);
                  // 切换 input type 后恢复光标 / 选区：视线与光标都留在密码里，
                  // 而不是跳到旁边的小眼睛按钮上。
                  window.requestAnimationFrame(() => {
                    const next = passwordInput.current;
                    if (!next) return;
                    next.focus();
                    if (start !== null && end !== null) {
                      next.setSelectionRange(start, end);
                    }
                  });
                }}
              >
                {showPassword ? <IconEyeOff /> : <IconEye />}
              </button>
              <button
                type="submit"
                className="skin-login__submit"
                disabled={!password || busy}
              >
                {busy ? "登录中..." : "登录"}
              </button>
            </div>
          </>
        )}
      </AnimatedSwap>

      {error ? (
        <p className="account-form__error" role="alert">
          {error}
        </p>
      ) : null}
    </form>
  );
}

export default LittleSkinAccount;
