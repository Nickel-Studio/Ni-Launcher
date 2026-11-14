import { LittleSkinAccount } from "./LittleSkinAccount";
import { MicrosoftAccount } from "./MicrosoftAccount";
import { type Account, type AccountType, type Identity } from "./types";
import type { SkinAccount, SkinSession } from "./yggdrasil";
import "./account.css";

export interface AccountPanelProps {
  accountType: AccountType;
  identity: Identity;
  littleskinServer: string;
  /** 微软设备码登录成功后回传后端落库的公开账号（含 uuid）。 */
  onMicrosoftAccount: (account: Account) => void | Promise<void>;
  /** 皮肤站登录成功后回传后端落库的权威账号（含 uuid）。 */
  onSkinAccount: (account: SkinAccount) => void | Promise<void>;
  onLogout: () => void;
  /** 皮肤站返回多角色时上报会话（选角弹窗由首页统一挂载）。 */
  onProfileSelect: (session: SkinSession) => void;
}

/**
 * 账号表单切换器（无卡片外壳、无账号分段）：
 * 仅根据 `accountType` 渲染微软 / 皮肤站的登录表单；离线形态没有表单，
 * 首页只展示头像与名字，改名统一走设置页「离线用户名」。
 * 账号形态切换入口统一收敛到首页卡片顶部的胶囊，避免出现第二个入口。
 */
export function AccountPanel({
  accountType,
  identity,
  littleskinServer,
  onMicrosoftAccount,
  onSkinAccount,
  onLogout,
  onProfileSelect,
}: AccountPanelProps) {
  return (
    <div className="account-form-stack">
      {accountType === "microsoft" ? (
        <MicrosoftAccount
          identity={identity}
          onAccount={onMicrosoftAccount}
          onLogout={onLogout}
        />
      ) : null}

      {accountType === "littleskin" ? (
        <LittleSkinAccount
          identity={identity}
          server={littleskinServer}
          onAccount={onSkinAccount}
          onLogout={onLogout}
          onProfileSelect={onProfileSelect}
        />
      ) : null}
    </div>
  );
}

export default AccountPanel;
