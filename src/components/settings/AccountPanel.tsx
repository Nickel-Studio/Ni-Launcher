import { Switch, Tooltip } from "../ui";
import { SettingRow } from "./SettingRow";
import { SettingsGroup } from "./SettingsGroup";
import type { AppSettings } from "./types";

type Props = {
  settings: AppSettings;
  update: (patch: Partial<AppSettings>) => void;
};

const MICROSOFT_SSL_LABEL = "在正版登录时进行SSL证书验证";
const MICROSOFT_SSL_TOOLTIP =
  "开启验证可以提高安全性,但可能会导致正版登录失败。\n若登录因为“未能为SSL/TLS安全通道建立信任关系”失败,请考虑关闭此选项。";

/** B. 账号与隐私。 */
export function AccountPanel({ settings, update }: Props) {
  return (
    <SettingsGroup
      id="account"
      title="账号与隐私"
      description="登录凭据与本机隐私偏好"
    >
      <SettingRow
        label="隐私模式"
        description="隐藏邮箱等敏感信息，仅本机可见"
        control={
          <Switch
            aria-label="隐私模式"
            checked={settings.privacyMode}
            onChange={(v: boolean) => update({ privacyMode: v })}
          />
        }
      />

      <SettingRow
        label={MICROSOFT_SSL_LABEL}
        control={
          <Tooltip label={MICROSOFT_SSL_TOOLTIP}>
            <Switch
              aria-label={MICROSOFT_SSL_LABEL}
              checked={settings.verifyMicrosoftSsl}
              onChange={(checked) => update({ verifyMicrosoftSsl: checked })}
            />
          </Tooltip>
        }
      />

      <SettingRow
        stacked
        label="离线用户名"
        description="离线进入游戏的昵称（最多 16 个字符）"
        control={
          <input
            className="st-input"
            value={settings.offlineUsername ?? ""}
            maxLength={16}
            spellCheck={false}
            autoComplete="off"
            placeholder="Player"
            onChange={(e) => update({ offlineUsername: e.target.value })}
          />
        }
      />

      <SettingRow
        stacked
        label="皮肤站服务器"
        description="第三方皮肤站地址，默认 LittleSkin"
        control={
          <input
            className="st-input"
            value={settings.littleskinServer ?? ""}
            spellCheck={false}
            autoComplete="off"
            inputMode="url"
            placeholder="https://littleskin.cn"
            onChange={(e) => update({ littleskinServer: e.target.value })}
          />
        }
      />
    </SettingsGroup>
  );
}
