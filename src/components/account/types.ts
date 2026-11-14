/**
 * 首页 · 账号/角色 相关的本地类型与纯工具函数。
 *
 * 注意：这里刻意**不**从 `src/lib/settings.ts` 导入 `AppSettings` 类型，
 * 而是使用结构化最小依赖（只接收需要的原始字段），
 * 以降低与 design-shell 契约的耦合，避免类型导出差异导致编译失败。
 */

/** 三种账号形态，复用 design-shell 的单一数据源类型。 */
import type { AccountType } from "../../lib/account";
export type { AccountType };

/** 当前生效的身份信息（由多账号列表派生）。 */
export interface Identity {
  /** 角色名 */
  name: string;
  /** 邮箱；离线形态为空字符串 */
  email: string;
  /** 是否已登录 / 已就绪 */
  loggedIn: boolean;
}

/** 账号形态的展示元信息。 */
export const ACCOUNT_META: Record<AccountType, { label: string }> = {
  microsoft: { label: "正版账号" },
  littleskin: { label: "皮肤站" },
  offline: { label: "离线" },
};

/**
 * 邮箱脱敏：`player@example.com` → `pl••••@••••.com`
 * 满足首页「隐私模式」要求，且对畸形输入安全兜底。
 */
export function maskEmail(email: string): string {
  const value = (email ?? "").trim();
  if (!value) return "••••";

  const at = value.lastIndexOf("@");
  if (at <= 0) {
    // 不是邮箱，退化为整体掩码
    return value.length > 2 ? `${value.slice(0, 2)}••••` : "••••";
  }

  const local = value.slice(0, at);
  const domain = value.slice(at + 1);

  const maskedLocal = local.length > 2 ? `${local.slice(0, 2)}••••` : "••••";

  const dot = domain.lastIndexOf(".");
  const tld = dot > 0 ? domain.slice(dot) : "";
  return `${maskedLocal}@••••${tld}`;
}

/** 头像加载失败时的兜底：取首字母（大写，非字母则用 "?"）。 */
export function avatarInitial(name: string): string {
  const ch = (name || "").trim().charAt(0);
  return ch ? ch.toUpperCase() : "?";
}

/**
 * 多账号列表中的一条账号记录（仅公开字段，凭据只由 Rust 侧落盘）。
 * 字段与 Rust 侧 `accounts_*` 命令契约保持一致（见任务说明）。
 */
export interface Account {
  id: string;
  type: AccountType;
  name: string;
  uuid?: string;
  email?: string;
  server?: string;
}

/** 账号形态的中文短标签。 */
export function accountTypeLabel(type: AccountType): string {
  return ACCOUNT_META[type]?.label ?? "账号";
}

/** 账号副标题：邮箱 / 皮肤站地址 / 本地离线。 */
export function accountSubtitle(account: Account): string {
  if (account.type === "littleskin") return account.server || "LittleSkin";
  if (account.type === "microsoft") return account.email || "Microsoft 账户";
  return "离线 · 仅本地";
}
