import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Modal } from "../ui";
import { SkinSiteIcon } from "./SkinSiteIcon";
import { showToast } from "../../lib/toast";
import { notifyLoginError } from "./loginFeedback";
import type { SkinAuthContext } from "../../lib/skinAuth";
import { isTauri } from "../../lib/settings";
import {
  setSkinSession,
  type SkinAccount,
  type SkinLoginOutcome,
  type SkinSession,
} from "./yggdrasil";
interface Props {
  context: SkinAuthContext;
  onAccount: (account: SkinAccount) => void | Promise<void>;
  onProfileSelect: (session: SkinSession) => void;
  disabled?: boolean;
}
export function SkinOAuthLogin({
  context,
  onAccount,
  onProfileSelect,
  disabled,
}: Props) {
  const [busy, setBusy] = useState(false);
  const session = useRef<string | null>(null);
  const pending = useRef(false);
  const generation = useRef(0);
  const scope = JSON.stringify([
    context.root,
    context.versionId,
    context.site.server,
    context.method,
  ]);
  const cancel = () => {
    if (!pending.current) return;
    generation.current++;
    void invoke("skin_oauth_cancel", { sessionId: session.current }).catch(
      () => undefined
    );
    session.current = null;
    pending.current = false;
    setBusy(false);
    showToast(`${context.site.name} 登录已取消`, "info");
  };
  useEffect(() => {
    setBusy(false);
    return () => {
      generation.current++;
      if (session.current)
        void invoke("skin_oauth_cancel", { sessionId: session.current }).catch(
          () => undefined
        );
      session.current = null;
      pending.current = false;
    };
  }, [scope]);
  const login = async () => {
    if (pending.current || disabled) return;
    if (!isTauri()) {
      showToast("请在桌面启动器中登录", "error");
      return;
    }
    pending.current = true;
    const current = ++generation.current;
    setBusy(true);
    try {
      const id = await invoke<string>("skin_oauth_begin", {
        root: context.root,
        versionId: context.versionId,
      });
      if (generation.current !== current) {
        void invoke("skin_oauth_cancel", { sessionId: id }).catch(
          () => undefined
        );
        return;
      }
      session.current = id;
      const outcome = await invoke<SkinLoginOutcome>("skin_oauth_finish", {
        sessionId: id,
      });
      if (generation.current !== current) return;
      session.current = null;
      setBusy(false);
      if (outcome.status === "account") {
        await onAccount(outcome.account);
      } else {
        const next = {
          sessionId: outcome.sessionId,
          profiles: outcome.profiles,
          oauthSiteName: context.site.name,
        };
        setSkinSession(next);
        onProfileSelect(next);
      }
    } catch (e) {
      if (generation.current === current) {
        notifyLoginError(context.site.name, e);
        setBusy(false);
        session.current = null;
      }
    } finally {
      if (generation.current === current) pending.current = false;
    }
  };
  return (
    <div className="account-form">
      <div className="account-form__row account-form__row--center">
        <Button
          variant="primary"
          className="skin-oauth-login__button"
          iconLeft={<SkinSiteIcon site={context.site} />}
          disabled={disabled || busy}
          onClick={() => void login()}
        >
          使用 {context.site.name} 登录
        </Button>
      </div>
      <Modal
        open={busy}
        onClose={cancel}
        title={`${context.site.name} 登录`}
        width={420}
        footer={
          <Button variant="secondary" onClick={cancel}>
            取消
          </Button>
        }
      >
        <p>请在浏览器中完成授权，完成后将自动返回启动器。</p>
      </Modal>
    </div>
  );
}
