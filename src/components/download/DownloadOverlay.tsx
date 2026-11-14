import { useEffect } from "react";
import { createPortal } from "react-dom";
import { DownloadBubble } from "./DownloadBubble";
import { connectDownloadCenter } from "./store";

/** App 全局只挂载一次；独立于路由过渡，不接受拖动或改变固定锚点。 */
export function DownloadOverlay() {
  useEffect(() => { void connectDownloadCenter(); }, []);
  return createPortal(<DownloadBubble />, document.body);
}
