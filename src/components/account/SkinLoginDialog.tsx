import { Modal } from "../ui";
import { LittleSkinAccount } from "./LittleSkinAccount";
import type { Identity } from "./types";
import type { SkinAccount, SkinSession } from "./yggdrasil";
import "./account.css";

export interface SkinLoginDialogProps {
  open: boolean;
  server: string;
  onClose: () => void;
  /** 登录成功后交回后端落库的权威账号（含 uuid），由首页入库并激活。 */
  onAccount: (account: SkinAccount) => void | Promise<void>;
  /** 出现多角色时交由首页弹出选角弹窗（同时关闭本弹窗，避免弹窗套弹窗）。 */
  onProfileSelect: (session: SkinSession) => void;
}

const EMPTY_IDENTITY: Identity = { name: "", email: "", loggedIn: false };

/**
 * 「变更角色」在**没有可用会话**时弹出的皮肤站登录弹窗。
 *
 * 复用与内联登录完全相同的两阶段表单（LittleSkinAccount），
 * 因此不存在第二套登录实现，两处的密码处理与多角色分支行为必然一致。
 */
export function SkinLoginDialog({
  open,
  server,
  onClose,
  onAccount,
  onProfileSelect,
}: SkinLoginDialogProps) {
  return (
    <Modal open={open} onClose={onClose} title="登录皮肤站" width={420}>
      <LittleSkinAccount
        identity={EMPTY_IDENTITY}
        server={server}
        onProfileSelect={onProfileSelect}
        onAccount={async (account) => {
          await onAccount(account);
          onClose();
        }}
      />
    </Modal>
  );
}

export default SkinLoginDialog;
