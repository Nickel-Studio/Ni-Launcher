import { Select, Slider } from "../ui";
import { MAX_DOWNLOAD_THREADS, MIB } from "../../lib/settings";
import { SettingRow } from "./SettingRow";
import { SettingsGroup } from "./SettingsGroup";
import type { AppSettings, SourcePolicy } from "../../lib/settings";
import { DOWNLOAD_SOURCE_OPTIONS as SOURCE_OPTIONS } from "./accents";

type Props = {
  settings: AppSettings;
  update: (patch: Partial<AppSettings>) => void;
};
const LIMIT_STEPS = 36;
const UNLIMITED_STEP = LIMIT_STEPS + 1;
const LIST_OPTIONS = SOURCE_OPTIONS.map((option) =>
  option.value === "mirror_first"
    ? { ...option, label: "尽量使用镜像源(可能缺少刚刚更新的版本)" }
    : option
);

export function limitFromStep(step: number): number {
  return step >= UNLIMITED_STEP ? 0 : Math.round((MIB / 4) * 2 ** (step / 4));
}
export function stepFromLimit(bps: number): number {
  return bps === 0
    ? UNLIMITED_STEP
    : Math.max(
        0,
        Math.min(LIMIT_STEPS, Math.round(4 * Math.log2(bps / (MIB / 4))))
      );
}
function formatLimit(step: number): string {
  const bps = limitFromStep(step);
  return bps === 0 ? "不限速" : `${Number((bps / MIB).toFixed(2))} MiB/s`;
}

export function DownloadPanel({ settings, update }: Props) {
  return (
    <SettingsGroup
      id="download"
      title="下载"
      description="下载来源、并发与速度限制"
    >
      <SettingRow
        label="版本列表源"
        fill
        control={
          <Select
            value={settings.versionListSource}
            ariaLabel="版本列表源"
            options={LIST_OPTIONS}
            onChange={(value) =>
              update({ versionListSource: value as SourcePolicy })
            }
          />
        }
      />
      <SettingRow
        label="文件下载源"
        fill
        control={
          <Select
            value={settings.fileDownloadSource}
            ariaLabel="文件下载源"
            options={SOURCE_OPTIONS}
            onChange={(value) =>
              update({ fileDownloadSource: value as SourcePolicy })
            }
          />
        }
      />
      <SettingRow
        label="最大线程数"
        fill
        control={
          <Slider
            min={1}
            max={MAX_DOWNLOAD_THREADS}
            value={settings.maxDownloadThreads}
            aria-label="最大线程数"
            onChange={(value) => update({ maxDownloadThreads: value })}
          />
        }
      />
      <SettingRow
        label="速度限制"
        fill
        control={
          <Slider
            min={0}
            max={UNLIMITED_STEP}
            value={stepFromLimit(settings.downloadLimitBps)}
            aria-label="速度限制"
            formatValue={formatLimit}
            onChange={(value) =>
              update({ downloadLimitBps: limitFromStep(value) })
            }
          />
        }
      />
      <SettingRow
        label="社区资源源"
        fill
        control={
          <Select
            value={settings.communityApiSource}
            ariaLabel="社区资源源"
            options={SOURCE_OPTIONS}
            onChange={(value) =>
              update({ communityApiSource: value as SourcePolicy })
            }
          />
        }
      />
    </SettingsGroup>
  );
}
