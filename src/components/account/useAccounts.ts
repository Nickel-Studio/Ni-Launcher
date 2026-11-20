import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";
import { type Account, type AccountType } from "./types";

const LIST_KEY = "nickel.accounts";
const ACTIVE_KEY = "nickel.accounts.active";

/* --------------------------------------------------------- normalization */
function normalizeAccount(raw: unknown): Account | null {
  if (!raw || typeof raw !== "object") return null;
  const item = raw as Record<string, unknown>;
  const id = typeof item.id === "string" ? item.id : "";
  const type = item.type as AccountType;
  if (!id) return null;
  if (type !== "microsoft" && type !== "littleskin" && type !== "offline") {
    return null;
  }
  return {
    id,
    type,
    name: typeof item.name === "string" ? item.name : "",
    uuid: typeof item.uuid === "string" ? item.uuid : undefined,
    email: typeof item.email === "string" ? item.email : undefined,
    server: typeof item.server === "string" ? item.server : undefined,
  };
}

function normalizeList(raw: unknown): Account[] {
  if (!Array.isArray(raw)) return [];
  return raw.map(normalizeAccount).filter((a): a is Account => a !== null);
}

/* ------------------------------------------------------------- localStorage */
function readLocalList(): Account[] {
  try {
    const raw = window.localStorage.getItem(LIST_KEY);
    if (!raw) return [];
    return normalizeList(JSON.parse(raw));
  } catch {
    return [];
  }
}

function readLocalActive(): string {
  try {
    return window.localStorage.getItem(ACTIVE_KEY) ?? "";
  } catch {
    return "";
  }
}

function writeLocal(list: Account[], activeId: string): void {
  try {
    // Account 只含公开字段（不含 accessToken 等凭据），这里不会持久化令牌。
    window.localStorage.setItem(LIST_KEY, JSON.stringify(list));
    window.localStorage.setItem(ACTIVE_KEY, activeId);
  } catch {
    /* storage disabled — keep memory state only */
  }
}

/* -------------------------------------------------------------------- hook */
export interface UseAccounts {
  accounts: Account[];
  activeId: string;
  loading: boolean;
  /** 写入结果：true = 已持久化（非 Tauri 视为本地已写入）；false = 命令失败。 */
  upsert: (account: Account) => Promise<boolean>;
  remove: (id: string) => Promise<void>;
  /** 激活结果：true = 已持久化（非 Tauri 视为本地已写入）；false = 命令失败。 */
  setActive: (id: string) => Promise<boolean>;
}

/**
 * 多账号（正版 / 皮肤站 / 离线）状态。
 * 优先使用 Rust 命令 `accounts_list / accounts_upsert / accounts_remove /
 * accounts_set_active` 持久化；命令缺失（浏览器预览或后端未编译进来）时
 * 自动降级为 localStorage + 内存态，绝不抛错、界面始终可用。
 */
export function useAccounts(): UseAccounts {
  const [accounts, setAccounts] = useState<Account[]>(() => readLocalList());
  const [activeId, setActiveId] = useState<string>(() => readLocalActive());
  const [loading, setLoading] = useState(true);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // 初次加载：Tauri 优先，失败回落到本地。
  useEffect(() => {
    let alive = true;
    if (!isTauri()) {
      setLoading(false);
      return () => {
        alive = false;
      };
    }
    invoke<unknown>("accounts_list")
      .then((list) => {
        if (!alive) return;
        const parsed = normalizeList(list);
        setAccounts(parsed);
        setActiveId((prev) => {
          const next = parsed.some((a) => a.id === prev)
            ? prev
            : parsed[0]?.id ?? "";
          writeLocal(parsed, next);
          return next;
        });
      })
      .catch(() => {
        /* 命令缺失：保留 localStorage 状态 */
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, []);

  const upsert = useCallback(
    async (account: Account) => {
      // 乐观更新本地，保证 UI 即时响应。
      setAccounts((prev) => {
        const idx = prev.findIndex((a) => a.id === account.id);
        const next =
          idx >= 0
            ? prev.map((a) => (a.id === account.id ? account : a))
            : [...prev, account];
        writeLocal(next, activeId);
        return next;
      });
      if (!isTauri()) return true;
      try {
        const list = await invoke<unknown>("accounts_upsert", { account });
        const parsed = normalizeList(list);
        if (mounted.current && parsed.length > 0) {
          setAccounts(parsed);
          writeLocal(parsed, activeId);
        }
        return true;
      } catch {
        // 命令缺失或落盘失败：本地乐观更新保留（界面可用），
        // 由调用方根据 false 提示检查配置目录写入权限。
        return false;
      }
    },
    [activeId]
  );

  const remove = useCallback(
    async (id: string) => {
      setAccounts((prev) => {
        const next = prev.filter((a) => a.id !== id);
        writeLocal(next, activeId === id ? "" : activeId);
        return next;
      });
      setActiveId((prev) => (prev === id ? "" : prev));
      if (!isTauri()) return;
      try {
        const list = await invoke<unknown>("accounts_remove", { id });
        const parsed = normalizeList(list);
        if (mounted.current && parsed.length >= 0) {
          setAccounts(parsed);
          writeLocal(parsed, readLocalActive());
        }
      } catch {
        /* ignore */
      }
    },
    [activeId]
  );

  const setActive = useCallback(async (id: string) => {
    setActiveId(id);
    setAccounts((prev) => {
      writeLocal(prev, id);
      return prev;
    });
    if (!isTauri()) return true;
    try {
      await invoke("accounts_set_active", { id });
      return true;
    } catch {
      // 激活状态未能写入：本地状态已更新，由调用方决定如何提示。
      return false;
    }
  }, []);

  return { accounts, activeId, loading, upsert, remove, setActive };
}
