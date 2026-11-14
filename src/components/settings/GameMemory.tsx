import { useEffect, useRef, useState } from "react";
import { isTauri } from "../../lib/settings";
import { loadGameRuntimeInfo, type GameRuntimeInfo } from "./gameRuntime";

type Props = {
  /** 首屏 / 实例页覆盖值；运行时轮询成功后以最新系统值为准。 */
  totalMemoryMb?: number | null;
  availableMemoryMb?: number | null;
  suggestedMemoryMb?: number | null;
  /** 实例页的显式分配值；为 null 时使用建议分配。 */
  allocatedMemoryMb?: number | null;
};

const MB_PER_GB = 1024;
const POLL_INTERVAL_MS = 1000;
const PERCENT_MAX = 100;

export function formatMemory(value: number | null): string {
  return value === null ? "—" : `${(value / MB_PER_GB).toFixed(1)} GB`;
}

function runtimeError(reason: unknown): string {
  if (reason instanceof Error && reason.message) return reason.message;
  if (typeof reason === "string" && reason) return reason;
  return "请稍后重试。";
}

function percentOf(value: number | null, total: number | null): number {
  if (total === null || total <= 0 || value === null) return 0;
  return Math.min(PERCENT_MAX, Math.max(0, (value / total) * PERCENT_MAX));
}

/** 隐藏页面暂停轮询；读取失败保留成功值，浏览器预览仅使用传入数据。 */
export function GameMemory({
  totalMemoryMb = null,
  availableMemoryMb = null,
  suggestedMemoryMb = null,
  allocatedMemoryMb = null,
}: Props) {
  const [runtime, setRuntime] = useState<GameRuntimeInfo | null>(null);
  const [error, setError] = useState("");
  const alive = useRef(true);
  const busy = useRef(false);

  useEffect(() => {
    alive.current = true;
    // 浏览器预览（非 Tauri）没有 game_runtime_info 命令：不轮询、不报错，
    // 仅用外部传入的首屏值渲染，避免每秒产生一次失败提示与重渲染。
    if (!isTauri()) {
      return () => {
        alive.current = false;
      };
    }
    let timer: number | undefined;

    const stop = () => {
      if (timer !== undefined) {
        window.clearInterval(timer);
        timer = undefined;
      }
    };

    const tick = async () => {
      if (busy.current || document.hidden) return;
      busy.current = true;
      try {
        const info = await loadGameRuntimeInfo();
        if (!alive.current) return;
        setRuntime(info);
        setError("");
      } catch (reason) {
        if (alive.current) {
          setError(`无法读取运行信息：${runtimeError(reason)}`);
        }
      } finally {
        busy.current = false;
      }
    };

    const sync = () => {
      if (document.hidden) {
        stop();
        return;
      }
      void tick();
      if (timer === undefined) {
        timer = window.setInterval(() => void tick(), POLL_INTERVAL_MS);
      }
    };

    sync();
    document.addEventListener("visibilitychange", sync);
    return () => {
      alive.current = false;
      stop();
      document.removeEventListener("visibilitychange", sync);
    };
  }, []);

  const total = runtime?.totalMemoryMb ?? totalMemoryMb;
  const safeTotal = total !== null && total > 0 ? total : null;
  const liveAvailable = runtime?.availableMemoryMb ?? availableMemoryMb;
  const available =
    safeTotal !== null && liveAvailable !== null
      ? Math.max(0, Math.min(safeTotal, liveAvailable))
      : null;
  const suggested = suggestedMemoryMb ?? runtime?.suggestedMemoryMb ?? null;
  const requested =
    allocatedMemoryMb !== null && Number.isFinite(allocatedMemoryMb)
      ? Math.max(0, allocatedMemoryMb)
      : null;
  const allocated = requested ?? suggested;
  const used =
    safeTotal !== null && available !== null
      ? Math.max(0, safeTotal - available)
      : null;
  const gameActual =
    allocated !== null && available !== null
      ? Math.min(allocated, available)
      : allocated;
  const empty =
    safeTotal !== null
      ? Math.max(0, safeTotal - (used ?? 0) - (gameActual ?? 0))
      : null;
  const usedPercent = percentOf(used, safeTotal);
  const allocatedPercent = percentOf(gameActual, safeTotal);
  const emptyPercent = percentOf(empty, safeTotal);
  const overAvailable =
    allocated !== null && available !== null && allocated > available;
  const barLabel = `已使用 ${formatMemory(used)}，游戏分配 ${formatMemory(
    gameActual
  )}，空闲 ${formatMemory(empty)}`;

  return (
    <div className="st-game-memory" aria-label="运行内存概览">
      <div className="st-game-memory__bar" role="img" aria-label={barLabel}>
        <span
          className="st-game-memory__segment st-game-memory__used"
          style={{ width: `${usedPercent}%` }}
        />
        <span
          className="st-game-memory__segment st-game-memory__allocated"
          style={{ width: `${allocatedPercent}%` }}
        />
        <span
          className="st-game-memory__segment st-game-memory__empty"
          style={{ width: `${emptyPercent}%` }}
        />
      </div>
      <dl className="st-game-memory__values">
        <div>
          <dt>已使用内存</dt>
          <dd>
            {formatMemory(used)} / {formatMemory(safeTotal)}
          </dd>
        </div>
        <div>
          <dt>游戏分配</dt>
          <dd>
            {formatMemory(allocated)}
            {overAvailable ? ` (可用 ${formatMemory(available)})` : ""}
          </dd>
        </div>
      </dl>
      {error ? (
        <p className="st-game-memory__error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
