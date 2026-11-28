import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { Button, Collapse, SegmentedControl } from "../ui";
import { isTauri } from "../../lib/settings";
import {
  isBuiltinSkinSite,
  siteFallback,
  type SkinAuthMethod,
  type SkinSite,
} from "../../lib/skinAuth";
import "./skinAuthSettings.css";
interface Config {
  clientId: string;
  clientSecret: string;
  source: string;
  redirectUri: string;
}
interface Props {
  server: string;
  method: SkinAuthMethod | null;
  globalMethod?: SkinAuthMethod;
  onChange: (method: SkinAuthMethod | null) => void;
  root?: string;
  versionId?: string;
}
const EMPTY: Config = {
  clientId: "",
  clientSecret: "",
  source: "empty",
  redirectUri: "nickel://auth",
};
const queues = new Map<string, Promise<unknown>>();
export function SkinAuthSettings(props: Props) {
  return (
    <SkinAuthSettingsForm
      key={JSON.stringify([props.server, props.root, props.versionId])}
      {...props}
    />
  );
}
function SkinAuthSettingsForm({
  server,
  method,
  globalMethod = "classic",
  onChange,
  root,
  versionId,
}: Props) {
  const [config, setConfig] = useState<Config>(EMPTY);
  const [site, setSite] = useState<SkinSite>(() => siteFallback(server));
  const builtin = isBuiltinSkinSite(server);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const draft = useRef(config);
  const effective = method ?? globalMethod;
  const key = JSON.stringify([server, root ?? null, versionId ?? null]);
  useEffect(() => {
    const current = ++generation.current;
    setError("");
    setReady(false);
    setConfig(EMPTY);
    draft.current = EMPTY;
    setSite(siteFallback(server));
    const timer = window.setTimeout(() => {
      if (!isTauri()) {
        setReady(true);
        return;
      }
      const args = { server, root: root ?? null, versionId: versionId ?? null };
      void (queues.get(key) ?? Promise.resolve())
        .catch(() => undefined)
        .then(() =>
          builtin ? EMPTY : invoke<Config>("skin_oauth_config_get", args)
        )
        .then((value) => {
          if (current === generation.current) {
            draft.current = value;
            setConfig(value);
            setReady(true);
          }
        })
        .catch((e) => {
          if (current === generation.current) {
            setReady(true);
            setError(String(e));
          }
        });
      void invoke<SkinSite>("skin_site_describe", { server, legacyName: null })
        .then((value) => {
          if (current === generation.current) setSite(value);
        })
        .catch(() => undefined);
    }, 350);
    return () => {
      generation.current++;
      window.clearTimeout(timer);
    };
  }, [key]);
  const persist = (next: Config | null) => {
    if (builtin) return;
    const current = generation.current;
    const pending = (queues.get(key) ?? Promise.resolve())
      .catch(() => undefined)
      .then(() => {
        if (!isTauri()) return;
        return invoke("skin_oauth_config_set", {
          server,
          root: root ?? null,
          versionId: versionId ?? null,
          config: next
            ? { clientId: next.clientId, clientSecret: next.clientSecret }
            : null,
        });
      });
    queues.set(key, pending);
    void pending
      .then(async () => {
        window.dispatchEvent(new Event("skin-auth-settings-changed"));
        if (next === null && isTauri()) {
          const value = await invoke<Config>("skin_oauth_config_get", {
            server,
            root: root ?? null,
            versionId: versionId ?? null,
          });
          if (current === generation.current) {
            setConfig(value);
            draft.current = value;
          }
        }
        if (current === generation.current) setError("");
      })
      .catch((e) => {
        if (current === generation.current) setError(String(e));
      })
      .finally(() => {
        if (queues.get(key) === pending) queues.delete(key);
      });
  };
  const edit = (patch: Partial<Config>) => {
    const next = { ...draft.current, ...patch, source: "custom" };
    draft.current = next;
    setConfig(next);
    persist(next);
  };
  return (
    <div className="skin-auth-settings">
      <p className="skin-auth-settings__caption" title={site.server}>
        {site.name}
      </p>
      <p className="skin-auth-settings__caption">偏好认证方式</p>
      <SegmentedControl
        value={method ?? "global"}
        onChange={(value) =>
          onChange(value === "global" ? null : (value as SkinAuthMethod))
        }
        options={[
          ...(root ? [{ value: "global", label: "跟随全局" }] : []),
          { value: "classic", label: "经典登录方式" },
          { value: "oauth2", label: "OAuth2 方式" },
        ]}
      />
      {!builtin && (
        <Collapse open={effective === "oauth2"} staggered>
          <div className="skin-auth-settings__fields">
            <p className="skin-auth-settings__caption">自定义 OAuth</p>
            <label>
              Client ID
              <input
                className="st-input"
                value={config.clientId}
                disabled={!ready}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => edit({ clientId: e.target.value })}
              />
            </label>
            <label>
              Client Secret
              <input
                className="st-input"
                type="password"
                value={config.clientSecret}
                disabled={!ready}
                autoComplete="new-password"
                spellCheck={false}
                onChange={(e) => edit({ clientSecret: e.target.value })}
              />
            </label>
            <label>
              回调 URI
              <input
                className="st-input"
                readOnly
                disabled
                value={config.redirectUri}
              />
            </label>
            {root && (
              <Button
                size="sm"
                variant="ghost"
                disabled={!ready}
                onClick={() => persist(null)}
              >
                恢复继承
              </Button>
            )}
            {error ? (
              <p role="alert">
                {error}
                <Button size="sm" onClick={() => persist(draft.current)}>
                  重试
                </Button>
              </p>
            ) : null}
          </div>
        </Collapse>
      )}
    </div>
  );
}
