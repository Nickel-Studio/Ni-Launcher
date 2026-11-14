import { useEffect, useMemo, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";
import {
  OFFLINE_HEADS,
  composeHead,
  mcHeadsUrl,
  randomOfflineHead,
  readAvatarCache,
  writeAvatarCache,
} from "./avatar";
import type { AccountType } from "./types";
import "./account.css";

export interface CharacterAvatarProps {
  name: string;
  size?: number;
  /** 账号形态：用于向皮肤站请求真实皮肤 */
  source?: AccountType;
  /** 皮肤站账号的 UUID 与站点根：后端据此读 Yggdrasil 档案里的真实皮肤。 */
  uuid?: string;
  server?: string;
  /** 刷新信号：数值变化时重新解析（头像变更 / 改名后强制刷新） */
  reloadKey?: number;
}

/** 远程读取失败依次使用本地缓存与内置头像；编辑入口由相邻控件提供。 */
export function CharacterAvatar({
  name,
  size = 64,
  source = "microsoft",
  uuid,
  server,
  reloadKey = 0,
}: CharacterAvatarProps) {
  const [fetched, setFetched] = useState<string | null>(null);
  const [index, setIndex] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const imgRef = useRef<HTMLImageElement | null>(null);

  const cached = useMemo(() => readAvatarCache(name), [name, reloadKey]);
  /**
   * mc-heads 只认识 Mojang 名字空间：皮肤站角色名在它那里一律返回 Steve 默认头。
   * 因此皮肤站与离线账号都不走这个候选，只信后端 `skin_fetch` 的结果。
   */
  const remoteUrl = useMemo(
    () =>
      source !== "littleskin" && source !== "offline" && name.trim()
        ? mcHeadsUrl(name, Math.max(size * 2, 160))
        : "",
    [name, size, source]
  );
  /** 内置兜底：每个角色名固定一次随机结果，切换名字时重新随机。 */
  const offline = useMemo(() => randomOfflineHead(), [name]);

  /**
   * 候选链：远程 → 本地缓存 → 内置兜底（另一张内置图兜住单张损坏）。
   *
   * `framed` 由来源决定，不能按 MIME 判断：Vite 会把体积较小的内置 PNG
   * 内联成 data URL，但它仍是满幅头图；相反，`fetched` / `cached` 里的
   * 合成头像虽然是 data URL，却已经自带 48/64 的透明留白。
   */
  const chain = useMemo(() => {
    const list: { src: string; framed: boolean }[] = [];
    if (fetched) list.push({ src: fetched, framed: false });
    if (remoteUrl) list.push({ src: remoteUrl, framed: true });
    if (cached && cached !== fetched) list.push({ src: cached, framed: false });
    list.push({ src: offline, framed: true });
    // 两张内置图都是随包资源：一张解码失败时换另一张，避免停在空白。
    for (const head of OFFLINE_HEADS) {
      if (head !== offline) list.push({ src: head, framed: true });
    }
    return list;
  }, [fetched, remoteUrl, cached, offline]);

  // 切换角色名 / 账号形态 / 刷新信号时重置解析链。
  // 刻意不动 `loaded`：同一张图仍在元素里时不会再触发 onLoad，
  // 清掉它会让头像永久停在 opacity:0（重复点同一账号后头像消失）。
  useEffect(() => {
    setFetched(null);
    setIndex(0);
  }, [name, source, reloadKey]);

  // 远程真实皮肤：仅在 Tauri 环境尝试，失败静默降级。
  useEffect(() => {
    const username = name.trim();
    if (!username || !isTauri() || source === "offline") return;
    let alive = true;
    /** 后端返回：`skin` 是完整皮肤（本组件裁头部），`avatar` 已是渲染头像。 */
    invoke<{
      dataBase64?: string;
      mime?: string;
      kind?: "skin" | "avatar";
    } | null>("skin_fetch", { username, source, uuid, server })
      .then((res) => {
        const b64 = res?.dataBase64;
        if (!alive || !b64) return;
        const dataUrl = `data:${res?.mime || "image/png"};base64,${b64}`;
        // `avatar` 是皮肤站已经渲染好的方块头像，只铺脸部区域、不再从皮肤图集裁切。
        const mode = res?.kind === "avatar" ? "avatar" : "skin";
        return composeHead(dataUrl, Math.max(size * 4, 256), mode)
          .then((head) => {
            if (!alive) return;
            writeAvatarCache(username, head);
            setFetched(head);
            setIndex(0);
          })
          .catch(() => undefined);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [name, source, size, uuid, server]);

  const candidate = chain[Math.min(index, chain.length - 1)] ?? {
    src: offline,
    framed: true,
  };
  const { src, framed } = candidate;

  const handleError = useCallback(() => {
    setIndex((prev) => Math.min(prev + 1, chain.length - 1));
  }, [chain.length]);

  useEffect(() => {
    setLoaded(false);
  }, [src]);

  // 已缓存图片（dataURL / 秒开图）可能不触发 onLoad，主动补一次，避免头像不显示。
  useEffect(() => {
    const el = imgRef.current;
    if (el && el.complete && el.naturalWidth > 0) setLoaded(true);
  }, [src]);

  return (
    <div
      className="char-avatar"
      style={{ ["--avatar-size" as string]: `${size}px` }}
    >
      <div className="char-avatar__frame">
        <img
          key={src}
          ref={imgRef}
          className="char-avatar__img"
          src={src}
          alt={`${name || "角色"} 的头像`}
          width={size}
          height={size}
          draggable={false}
          decoding="async"
          data-loaded={loaded}
          data-framed={framed || undefined}
          onLoad={() => setLoaded(true)}
          onError={handleError}
        />
      </div>
    </div>
  );
}

export default CharacterAvatar;
