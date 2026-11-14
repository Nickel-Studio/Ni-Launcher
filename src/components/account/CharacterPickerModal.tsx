import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Modal } from "../ui";
import { isTauri } from "../../lib/settings";
import { CharacterAvatar } from "./CharacterAvatar";
import {
  readCachedProfiles,
  toReadableError,
  writeCachedProfiles,
  type SkinProfile,
  type SkinProfileList,
} from "./yggdrasil";
import type { Account } from "./types";
import "./account.css";

export interface CharacterPickerModalProps {
  open: boolean;
  account: Account | null;
  onClose: () => void;
  /**
   * 选中角色：`sessionId` 来自 `yggdrasil_profiles`，后端据此完成令牌绑定，
   * 用户无需再输一次密码；失败时在弹窗内给出可读原因。
   */
  onSelect: (profile: SkinProfile, sessionId: string) => Promise<void>;
  /** 打开皮肤站角色页（与「关闭」相对的一侧）。 */
  onOpenSite: () => void;
}

/**
 * 「切换角色」弹窗。
 *
 * 先把缓存的角色列表画出来（头像按 uuid 懒加载），同时向后端请求完整列表：
 * 后端用本机加密保存的密码重新认证，返回 `availableProfiles` 与一个待选会话。
 * 选中角色走 `yggdrasil_select_profile`；任何形式的关闭都会调用
 * `yggdrasil_cancel`，让后端把账号回绑到原角色，不留下悬空的令牌。
 * 列表与头像都只是公开信息，不含令牌。
 */
export function CharacterPickerModal({
  open,
  account,
  onClose,
  onSelect,
  onOpenSite,
}: CharacterPickerModalProps) {
  const [profiles, setProfiles] = useState<SkinProfile[]>([]);
  const [busyId, setBusyId] = useState("");
  const [error, setError] = useState("");
  const [sessionId, setSessionId] = useState("");

  const accountId = account?.id ?? "";
  const currentId = (account?.uuid ?? "").replace(/-/g, "").toLowerCase();
  const canChoose = sessionId !== "";

  // 卸载时也要释放会话：后端会回绑原角色，避免账号令牌悬空。
  const sessionRef = useRef("");
  useEffect(() => {
    sessionRef.current = sessionId;
  }, [sessionId]);
  useEffect(
    () => () => {
      const pending = sessionRef.current;
      if (pending && isTauri()) {
        void invoke("yggdrasil_cancel", { sessionId: pending });
      }
    },
    []
  );

  useEffect(() => {
    if (!open) return;
    const cached = readCachedProfiles(accountId);
    setProfiles(cached);
    setBusyId("");
    setError("");
    setSessionId("");
    if (!accountId || !isTauri()) return;

    let alive = true;
    invoke<SkinProfileList>("yggdrasil_profiles", { id: accountId })
      .then((list) => {
        if (!alive) {
          // 请求返回前弹窗已关闭：立刻释放会话，不把未绑定令牌留在后端。
          void invoke("yggdrasil_cancel", { sessionId: list.sessionId });
          return;
        }
        if (list.profiles.length > 0) {
          writeCachedProfiles(accountId, list.profiles);
          setProfiles(list.profiles);
        }
        setSessionId(list.sessionId);
      })
      .catch((err) => {
        // 有缓存时也提示：否则用户会以为「点角色没反应」。
        if (alive) setError(toReadableError(err));
      });
    return () => {
      alive = false;
    };
  }, [open, accountId]);

  const close = () => {
    const pending = sessionId;
    setSessionId("");
    if (pending && isTauri()) {
      void invoke("yggdrasil_cancel", { sessionId: pending });
    }
    onClose();
  };

  const choose = async (profile: SkinProfile) => {
    if (busyId || !canChoose) return;
    setBusyId(profile.id);
    setError("");
    try {
      await onSelect(profile, sessionId);
      // 会话已被后端消费；这里不再 cancel。
      setSessionId("");
      onClose();
    } catch (err) {
      setError(toReadableError(err));
      setBusyId("");
    }
  };

  return (
    <Modal
      open={open}
      onClose={close}
      title="切换角色"
      width={420}
      footer={
        <>
          {/* 角色页与「关闭」分居页脚两端，避免两个动作挤在一起。 */}
          <Button
            variant="primary"
            className="character-picker__site"
            onClick={onOpenSite}
          >
            打开皮肤站角色页
          </Button>
          <Button variant="ghost" onClick={close}>
            关闭
          </Button>
        </>
      }
    >
      <div className="skin-profiles" role="listbox" aria-label="角色列表">
        {profiles.length === 0 ? (
          <p className="acc-switch__empty" role="status">
            {error || "正在读取角色…"}
          </p>
        ) : (
          profiles.map((profile) => {
            const active =
              profile.id.replace(/-/g, "").toLowerCase() === currentId;
            const busy = busyId === profile.id;
            return (
              <button
                key={profile.id}
                type="button"
                role="option"
                aria-selected={active}
                aria-current={active}
                data-selected={active}
                data-busy={busy}
                className="skin-profiles__row skin-profiles__row--avatar"
                disabled={busyId !== "" || !canChoose}
                onClick={() => void choose(profile)}
              >
                <CharacterAvatar
                  name={profile.name}
                  size={36}
                  source="littleskin"
                  uuid={profile.id}
                  server={account?.server}
                />
                <span className="skin-profiles__name">
                  {profile.name || "未命名角色"}
                </span>
                {active ? (
                  <span className="acc-switch__check">当前</span>
                ) : null}
              </button>
            );
          })
        )}
      </div>

      {error && profiles.length > 0 ? (
        <p className="account-form__error" role="alert">
          {error}
        </p>
      ) : null}
    </Modal>
  );
}

export default CharacterPickerModal;
