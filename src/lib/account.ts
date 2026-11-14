import type { ComponentType, SVGProps } from "react";
import { useCallback } from "react";
import { useSettings } from "./settings";
import { IconMicrosoft, IconSkin, IconUser } from "../components/ui/icons";

/** Account / login strategy used by the launcher. */
export type AccountType = "microsoft" | "littleskin" | "offline";

export interface AccountOption {
  id: AccountType;
  /** Short label, e.g. 正版 / 皮肤站 / 离线. */
  label: string;
  /** Secondary descriptive line. */
  desc: string;
  icon: ComponentType<SVGProps<SVGSVGElement>>;
}

/** Canonical ordering + metadata for the account selector (sidebar). */
export const ACCOUNT_OPTIONS: readonly AccountOption[] = [
  { id: "microsoft", label: "正版", desc: "Microsoft 账户登录", icon: IconMicrosoft },
  { id: "littleskin", label: "皮肤站", desc: "LittleSkin / 第三方皮肤站", icon: IconSkin },
  { id: "offline", label: "离线", desc: "使用离线用户名进入", icon: IconUser },
];

/**
 * Account-type binding shared between the home sidebar and the home page.
 *
 * The sidebar (rendered only on the home route) writes `settings.accountType`;
 * the home page reads the exact same field through this hook, so the two stay
 * in sync automatically — no extra prop plumbing required.
 */
export function useAccountType(): {
  accountType: AccountType;
  setAccountType: (type: AccountType) => void;
} {
  const { settings, update } = useSettings();
  const setAccountType = useCallback(
    (type: AccountType) => update({ accountType: type }),
    [update],
  );
  return { accountType: settings.accountType, setAccountType };
}
