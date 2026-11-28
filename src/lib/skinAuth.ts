import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri, useSettings } from "./settings";
import { useLaunchInstance } from "./instances";
export type SkinAuthMethod = "classic" | "oauth2";
export interface SkinSite {
  server: string;
  api: string;
  name: string;
  homepage: string;
  register: string;
}
export interface SkinAuthContext {
  exclusive: boolean;
  method: SkinAuthMethod;
  site: SkinSite;
  root: string | null;
  versionId: string | null;
}
export function skinSiteIdentity(raw: string): string {
  raw = raw.trim();
  if (/^https?:/i.test(raw) && !raw.includes("://")) return "";
  try {
    const url = new URL(raw.includes("://") ? raw : `https://${raw}`);
    if (
      !["http:", "https:"].includes(url.protocol) ||
      url.username ||
      url.password ||
      url.search ||
      url.hash
    )
      return "";
    return url.href.replace(/\/+$/, "").replace(/\/api\/yggdrasil$/, "");
  } catch {
    return "";
  }
}
export function sameSkinSite(a: string, b: string): boolean {
  const first = skinSiteIdentity(a);
  return first !== "" && first === skinSiteIdentity(b);
}
export function isBuiltinSkinSite(server: string): boolean {
  return !server.trim() || sameSkinSite(server, "https://littleskin.cn");
}
export function skinSiteFavicon(server: string): string {
  const identity = skinSiteIdentity(server.trim() || "https://littleskin.cn");
  return identity ? `${identity}/favicon.png` : "";
}
export function siteFallback(server: string): SkinSite {
  const identity = skinSiteIdentity(server);
  let name = "皮肤站";
  try {
    name = new URL(identity).hostname;
  } catch {
    /* Incomplete input. */
  }
  return {
    server,
    api: identity ? `${identity}/api/yggdrasil` : "",
    name,
    homepage: identity,
    register: "",
  };
}
export function useSkinAuthContext() {
  const { settings } = useSettings();
  const { instance } = useLaunchInstance();
  const [revision, setRevision] = useState(0);
  const [context, setContext] = useState<SkinAuthContext>({
    exclusive: false,
    method: settings.skinAuthMethod,
    site: siteFallback(settings.littleskinServer),
    root: null,
    versionId: null,
  });
  const [ready, setReady] = useState(!isTauri());
  useEffect(() => {
    const changed = () => setRevision((n) => n + 1);
    window.addEventListener("skin-auth-settings-changed", changed);
    return () =>
      window.removeEventListener("skin-auth-settings-changed", changed);
  }, []);
  useEffect(() => {
    let alive = true;
    setReady(!isTauri());
    if (!isTauri()) {
      setContext({
        exclusive: false,
        method: settings.skinAuthMethod,
        site: siteFallback(settings.littleskinServer),
        root: instance?.root ?? null,
        versionId: instance?.versionId ?? null,
      });
      return;
    }
    void invoke("skin_oauth_cancel", { sessionId: null }).catch(
      () => undefined
    );
    void invoke<SkinAuthContext>("skin_auth_context", {
      root: instance?.root ?? null,
      versionId: instance?.versionId ?? null,
    })
      .then((value) => {
        if (alive) {
          setContext(value);
          setReady(true);
          if (value.site.server)
            void invoke<SkinSite>("skin_site_describe", {
              server: value.site.server,
              legacyName: value.site.name,
            })
              .then((site) => {
                if (alive) setContext({ ...value, site });
              })
              .catch(() => undefined);
        }
      })
      .catch(() => {
        if (alive) setReady(false);
      });
    return () => {
      alive = false;
    };
  }, [
    instance?.root,
    instance?.versionId,
    settings.littleskinServer,
    settings.skinAuthMethod,
    revision,
  ]);
  return { context, ready };
}
