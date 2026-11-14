import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Modal } from "../ui";
import { isTauri } from "../../lib/settings";
import { CharacterAvatar } from "./CharacterAvatar";
import {
  toReadableError,
  type SkinAccount,
  type SkinProfile,
} from "./yggdrasil";
import "./account.css";

export interface SkinProfileDialogProps {
  open: boolean;
  sessionId: string;
  profiles: SkinProfile[];
  /** 皮肤站站点根：用于按 uuid 读取角色皮肤头像。 */
  server?: string;
  /** 选角成功：后端返回最终账号。 */
  onResolved: (account: SkinAccount) => void;
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
  onCancelled,
}: SkinProfileDialogProps) {
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!open) return;
    setSelected(profiles[0]?.id ?? "");
    setBusy(false);
    setError("");
  }, [open, sessionId, profiles]);

  const cancel = async () => {
    if (isTauri() && sessionId) {
      try {
        await invoke("yggdrasil_cancel", { sessionId });
      } catch {
        /* 会话可能已过期：取消失败不影响本地状态收敛 */
      }
    }
    onCancelled();
  };

  const confirm = async () => {
    if (!selected || busy) return;
    if (!isTauri()) {
      setError("当前环境不支持登录（请在桌面应用中运行）");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const account = await invoke<SkinAccount>("yggdrasil_select_profile", {
        sessionId,
        profileId: selected,
      });
      onResolved(account);
    } catch (err) {
      setError(toReadableError(err));
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={() => void cancel()}
      title="选择角色"
      width={420}
      footer={
        <>
          <Button variant="ghost" onClick={() => void cancel()}>
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

      {error ? (
        <p className="account-form__error" role="alert">
          {error}
        </p>
      ) : null}
    </Modal>
  );
}

export default SkinProfileDialog;
