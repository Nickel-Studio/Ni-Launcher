import { returnFromDownloads, setRoute } from "../../lib/nav";

export function openDownloadDetail(): void {
  setRoute("downloads");
}

/** Return to the route that opened downloads, not a fixed tab. */
export function closeDownloadDetail(): void {
  returnFromDownloads();
}
