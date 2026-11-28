import { useEffect, useMemo, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../../lib/settings";
import { skinSiteIdentity } from "../../lib/skinAuth";
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

/** 远程读取失败依次使用同站缓存与内置头像。身份变化同步丢弃旧图和在途结果。 */
export function CharacterAvatar(props: CharacterAvatarProps) {
  const source = props.source ?? "microsoft";
  const scope = JSON.stringify([
    source,
    source === "littleskin"
      ? skinSiteIdentity(props.server ?? "") || props.server || ""
      : "",
    (props.uuid ?? "").replace(/-/g, "").toLowerCase(),
  ]);
  return (
    <CharacterAvatarContent
      key={JSON.stringify([scope, props.name])}
      {...props}
      cacheScope={scope}
    />
  );
}

function CharacterAvatarContent({
  name,
  size = 64,
  source = "microsoft",
  uuid,
  server,
  reloadKey = 0,
  cacheScope,
}: CharacterAvatarProps & { cacheScope: string }) {
  const [fetched, setFetched] = useState<string | null>(null);
  const [index, setIndex] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const cached = useMemo(
    () => readAvatarCache(name, cacheScope),
    [name, cacheScope, reloadKey]
  );
  // mc-heads only knows Mojang identities, never third-party or offline profiles.
  const remoteUrl = useMemo(
    () =>
      source !== "littleskin" && source !== "offline" && name.trim()
        ? mcHeadsUrl(name, Math.max(size * 2, 160))
        : "",
    [name, size, source]
  );
  const offline = useMemo(() => randomOfflineHead(), [name]);
  const chain = useMemo(() => {
    const list: { src: string; framed: boolean }[] = [];
    if (fetched) list.push({ src: fetched, framed: false });
    if (remoteUrl) list.push({ src: remoteUrl, framed: true });
    if (cached && cached !== fetched) list.push({ src: cached, framed: false });
    list.push({ src: offline, framed: true });
    for (const head of OFFLINE_HEADS) {
      if (head !== offline) list.push({ src: head, framed: true });
    }
    return list;
  }, [fetched, remoteUrl, cached, offline]);

  useEffect(() => {
    setFetched(null);
    setIndex(0);
  }, [reloadKey]);

  useEffect(() => {
    const username = name.trim();
    if (!username || !isTauri() || source === "offline") return;
    let alive = true;
    invoke<{
      dataBase64?: string;
      mime?: string;
      kind?: "skin" | "avatar";
    } | null>("skin_fetch", { username, source, uuid, server })
      .then((res) => {
        const b64 = res?.dataBase64;
        if (!alive || !b64) return;
        const dataUrl = `data:${res?.mime || "image/png"};base64,${b64}`;
        const mode = res?.kind === "avatar" ? "avatar" : "skin";
        return composeHead(dataUrl, Math.max(size * 4, 256), mode)
          .then((head) => {
            if (!alive) return;
            writeAvatarCache(username, head, cacheScope);
            setFetched(head);
            setIndex(0);
          })
          .catch(() => undefined);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [name, source, size, uuid, server, cacheScope, reloadKey]);

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
  // A cached image may already be decoded before its load handler is attached.
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
