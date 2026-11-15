import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "./settings";
import { PRODUCT_VERSION } from "./brand";
import changelog from "../../CHANGELOG.json";

export interface OnboardingStatus {
  eulaVersion: string;
  eulaUrl: string;
  eulaAcceptedAt: number | null;
  currentVersion: string;
  needsEula: boolean;
  needsChangelog: boolean;
  exiting: boolean;
}

export const BUILTIN_CHANGELOG = changelog;
export const REJECTION_SECONDS = 5;
const PREVIEW_STORAGE = "nickel-onboarding-preview";
const PREVIEW_EULA = "minecraft-eula-v1";

function isUnixTimestamp(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

function validateStatus(value: unknown): OnboardingStatus {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("首次启动记录无效，请重试。");
  }
  const status = value as Partial<OnboardingStatus>;
  if (status.currentVersion !== BUILTIN_CHANGELOG.version) {
    throw new Error("更新说明与程序版本不一致，请重新安装当前版本。");
  }
  if (
    typeof status.eulaVersion !== "string" ||
    !status.eulaVersion ||
    typeof status.eulaUrl !== "string" ||
    status.eulaUrl !== "https://www.minecraft.net/zh-hans/eula" ||
    typeof status.needsEula !== "boolean" ||
    typeof status.needsChangelog !== "boolean" ||
    typeof status.exiting !== "boolean" ||
    (status.eulaAcceptedAt !== null &&
      !isUnixTimestamp(status.eulaAcceptedAt)) ||
    (!status.needsEula && !isUnixTimestamp(status.eulaAcceptedAt))
  ) {
    throw new Error("首次启动记录无效，请重试。");
  }
  return status as OnboardingStatus;
}

function previewStatus(): OnboardingStatus {
  let saved: Record<string, unknown> = {};
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(PREVIEW_STORAGE) || "{}"
    );
    if (value && typeof value === "object" && !Array.isArray(value)) {
      saved = value as Record<string, unknown>;
    }
  } catch {
    // Browser preview storage is separate from the desktop acceptance record.
  }
  const acceptedAt = isUnixTimestamp(saved.acceptedAt)
    ? saved.acceptedAt
    : null;
  const accepted = saved.eulaVersion === PREVIEW_EULA && acceptedAt !== null;
  return {
    eulaVersion: PREVIEW_EULA,
    eulaUrl: "https://www.minecraft.net/zh-hans/eula",
    eulaAcceptedAt: accepted ? acceptedAt : null,
    currentVersion: PRODUCT_VERSION,
    needsEula: !accepted,
    needsChangelog: saved.seenVersion !== PRODUCT_VERSION,
    exiting: false,
  };
}

export async function readOnboarding(): Promise<OnboardingStatus> {
  return validateStatus(
    isTauri() ? await invoke("onboarding_get") : previewStatus()
  );
}

export async function acceptEula(
  status: OnboardingStatus,
  checked: boolean
): Promise<OnboardingStatus> {
  validateStatus(status);
  if (!checked) throw new Error("请先勾选阅读声明。");
  if (status.exiting) throw new Error("正在退出，无法签署声明。");
  if (isTauri()) {
    return validateStatus(
      await invoke("onboarding_accept_eula", {
        checked,
        eulaVersion: status.eulaVersion,
      })
    );
  }
  const current = previewStatus();
  if (status.eulaVersion !== current.eulaVersion) {
    throw new Error("条款修订已变化，请重新读取确认记录。");
  }
  const acceptedAt = current.needsEula
    ? Math.floor(Date.now() / 1000)
    : current.eulaAcceptedAt;
  if (!isUnixTimestamp(acceptedAt))
    throw new Error("无法获取有效的签署时间，请检查系统时间后重试。");
  localStorage.setItem(
    PREVIEW_STORAGE,
    JSON.stringify({
      eulaVersion: current.eulaVersion,
      acceptedAt,
      seenVersion: current.needsChangelog ? undefined : PRODUCT_VERSION,
    })
  );
  return validateStatus(previewStatus());
}

export async function acknowledgeChangelog(
  status: OnboardingStatus
): Promise<OnboardingStatus> {
  validateStatus(status);
  if (status.exiting || status.needsEula)
    throw new Error("请先同意 Minecraft EULA。");
  if (isTauri()) {
    return validateStatus(
      await invoke("onboarding_ack_changelog", {
        version: status.currentVersion,
      })
    );
  }
  const current = previewStatus();
  if (current.needsEula) throw new Error("请先同意 Minecraft EULA。");
  localStorage.setItem(
    PREVIEW_STORAGE,
    JSON.stringify({
      eulaVersion: current.eulaVersion,
      acceptedAt: current.eulaAcceptedAt,
      seenVersion: current.currentVersion,
    })
  );
  return validateStatus(previewStatus());
}

export async function finishOnboarding(): Promise<void> {
  if (isTauri()) await invoke("onboarding_finish");
}

export async function rejectEula(): Promise<void> {
  if (isTauri()) await invoke("onboarding_reject");
}

export async function closeAfterRejection(): Promise<void> {
  if (isTauri()) await invoke("window_close");
}

export function rejectionSecondsRemaining(
  deadline: number,
  now: number
): number {
  return Math.max(
    0,
    Math.min(REJECTION_SECONDS, Math.ceil((deadline - now) / 1000))
  );
}
