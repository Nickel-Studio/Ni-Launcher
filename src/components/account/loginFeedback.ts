import { showToast } from "../../lib/toast";
import { toReadableError } from "./yggdrasil";

/** Only explicit provider denials are cancellations; timeouts remain failures. */
export function notifyLoginError(provider: string, error: unknown): void {
  const message = toReadableError(error);
  if (message === "你已在网页中取消授权" || message === "用户已取消登录") {
    showToast(`${provider} 登录已取消`, "info");
  } else {
    showToast(`${provider} 登录失败：${message}`, "error");
  }
}
