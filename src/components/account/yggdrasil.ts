/**
 * 皮肤站（Yggdrasil）协议契约：命令返回值类型、错误文案与会话存储。
 *
 * 为什么单独成文件：登录表单（LittleSkinAccount）与选角弹窗（SkinProfileDialog）
 * 都要用到同一份类型与同一段会话状态，集中在这里可以避免两边各写一套结构，
 * 也避免「弹窗在登录表单卸载后拿不到 sessionId」。
 */

/** `yggdrasil_login` / `yggdrasil_select_profile` 返回的账号（后端已剥离敏感字段）。 */
export interface SkinAccount {
  id: string;
  type: string;
  name: string;
  uuid?: string | null;
  email?: string | null;
  server?: string | null;
}

/** 多角色会话中的一个角色。 */
export interface SkinProfile {
  id: string;
  name: string;
}

/**
 * `yggdrasil_profiles` 的返回值：完整角色列表 + 本次待选会话。
 *
 * 会话由后端持有「未绑定角色」的访问令牌；选角时不必再输一次密码，
 * 关闭弹窗时前端调用 `yggdrasil_cancel` 让后端把账号回绑到原角色。
 */
export interface SkinProfileList {
  sessionId: string;
  profiles: SkinProfile[];
}

/**
 * `yggdrasil_login` 的返回值：包了一层的 LoginOutcome。
 * status = "account" → 直接拿到账号；status = "select" → 需要用户先选角色。
 */
export type SkinLoginOutcome =
  | { status: "account"; account: SkinAccount }
  | { status: "select"; sessionId: string; profiles: SkinProfile[] };

/** 待选角色会话。 */
export interface SkinSession {
  sessionId: string;
  profiles: SkinProfile[];
}

/**
 * 当前多角色会话（**仅内存**）。
 *
 * 为什么不做持久化：sessionId 由后端进程持有、随时可能过期，写进 localStorage
 * 只会在下次启动时给出一个必然失效的会话。进程结束后 getSkinSession() 返回 null，
 * 「变更角色」自动回退为重新登录，这正是我们想要的兜底行为。
 */
let current: SkinSession | null = null;

export function getSkinSession(): SkinSession | null {
  return current;
}

export function setSkinSession(session: SkinSession | null): void {
  current = session;
}

/**
 * 角色列表缓存键：皮肤站的 `availableProfiles` 只在登录时返回一次，
 * 缓存后「切换角色」弹窗可以立刻渲染（头像按 uuid 另行加载），不再等网络。
 * 缓存只含公开的 id / name，不含任何令牌。
 */
const PROFILE_KEY_PREFIX = "nowhere.skinProfiles.";

export function readCachedProfiles(accountId: string): SkinProfile[] {
  if (!accountId) return [];
  try {
    const raw = window.localStorage.getItem(
      `${PROFILE_KEY_PREFIX}${accountId}`
    );
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is SkinProfile => {
      const value = item as Partial<SkinProfile> | null;
      return typeof value?.id === "string" && typeof value?.name === "string";
    });
  } catch {
    return [];
  }
}

export function writeCachedProfiles(
  accountId: string,
  profiles: SkinProfile[]
): void {
  if (!accountId || profiles.length === 0) return;
  try {
    window.localStorage.setItem(
      `${PROFILE_KEY_PREFIX}${accountId}`,
      JSON.stringify(profiles)
    );
  } catch {
    /* 存储禁用 / 超额：忽略，不影响切换角色 */
  }
}

/** 把后端 / 运行时抛出的值统一成可读文案（后端已把 Yggdrasil 错误映射为中文）。 */
export function toReadableError(err: unknown): string {
  if (typeof err === "string" && err.trim()) return err;
  if (err instanceof Error && err.message) return err.message;
  return "登录失败，请稍后重试";
}
