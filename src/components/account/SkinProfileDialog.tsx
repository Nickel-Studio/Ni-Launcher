import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Modal } from "../ui";
import { isTauri } from "../../lib/settings";
import { showToast } from "../../lib/toast";
import { notifyLoginError } from "./loginFeedback";
import { CharacterAvatar } from "./CharacterAvatar";
import { type SkinAccount, type SkinProfile } from "./yggdrasil";
import "./account.css";

export interface SkinProfileDialogProps {
  open: boolean;
  sessionId: string;
  profiles: SkinProfile[];
  /** 皮肤站站点根：用于按 uuid 读取角色皮肤头像。 */
  server?: string;
  /** 选角成功：后端返回最终账号。 */
  onResolved: (account: SkinAccount) => void | Promise<void>;
  oauthSiteName?: string;
  /** 取消 / 关闭（已向后端取消该会话）。 */
  onCancelled: () => void;
}

/**
 * 多角色单选弹窗：`yggdrasil_login` 返回 status="select" 时使用。
 *
 * 单选后点「确认」调用 `yggdrasil_select_profile`；任何形式的关闭
 * （取消按钮 / 右上角 / Esc / 点遮罩）都会调用 `yggdrasil_cancel` 释放后端会话，
 * 不留悬挂的登录会话。
 */
export function SkinProfileDialog({
  open,
  sessionId,
  profiles,
  server,
  onResolved,
  oauthSiteName,
  onCancelled,
}: SkinProfileDialogProps) {
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false);

  const generation = useRef(0);
  const resolved = useRef(false);
  const pending = useRef(false);
  const closed = useRef(false);
  useEffect(() => {
    if (!open) return;
    resolved.current = false;
    pending.current = false;
    closed.current = false;
    setSelected(profiles[0]?.id ?? "");
    setBusy(false);
    return () => {
      generation.current++;
      if (!resolved.current && isTauri() && sessionId)
        void invoke("yggdrasil_cancel", { sessionId }).catch(() => undefined);
    };
  }, [open, sessionId, profiles]);

  const cancel = () => {
    if (closed.current || resolved.current) return;
    closed.current = true;
    generation.current++;
    if (isTauri() && sessionId)
      void invoke("yggdrasil_cancel", { sessionId }).catch(() => undefined);
    onCancelled();
    showToast(`${oauthSiteName || "皮肤站"} 登录已取消`, "info");
  };

  const confirm = async () => {
    if (!selected || pending.current || closed.current || resolved.current)
      return;
    if (!isTauri()) {
      showToast("当前环境不支持登录（请在桌面应用中运行）", "error");
      return;
    }
    const current = ++generation.current;
    pending.current = true;
    setBusy(true);
    try {
      const account = await invoke<SkinAccount>("yggdrasil_select_profile", {
        sessionId,
        profileId: selected,
      });
      if (current !== generation.current) return;
      resolved.current = true;
      await onResolved(account);
    } catch (err) {
      if (current !== generation.current) return;
      notifyLoginError(oauthSiteName || "皮肤站", err);
      if (resolved.current) onCancelled();
    } finally {
      if (current === generation.current) {
        pending.current = false;
        setBusy(false);
      }
    }
  };

  return (
    <Modal
      open={open}
      dismissible={!busy || !resolved.current}
      onClose={cancel}
      title="选择角色"
      width={420}
      footer={
        <>
          <Button
            variant="ghost"
            disabled={busy && resolved.current}
            onClick={cancel}
          >
            取消
          </Button>
          <Button
            variant="primary"
            disabled={!selected || busy}
            loading={busy}
            onClick={() => void confirm()}
          >
            确认
          </Button>
        </>
      }
    >
      <div className="skin-profiles" role="radiogroup" aria-label="角色列表">
        {profiles.map((profile) => {
          const active = profile.id === selected;
          return (
            <button
              key={profile.id}
              type="button"
              role="radio"
              aria-checked={active}
              data-selected={active}
              className="skin-profiles__row"
              onClick={() => setSelected(profile.id)}
            >
              <CharacterAvatar
                name={profile.name}
                size={36}
                source="littleskin"
                uuid={profile.id}
                server={server}
              />
              <span className="skin-profiles__name">
                {profile.name || "未命名角色"}
              </span>
              {active ? <span className="acc-switch__check">已选</span> : null}
            </button>
          );
        })}
      </div>
    </Modal>
  );
}

export default SkinProfileDialog;
