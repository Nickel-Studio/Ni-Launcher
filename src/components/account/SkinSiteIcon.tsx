import { useState } from "react";
import { skinSiteFavicon, type SkinSite } from "../../lib/skinAuth";
import "./account.css";

export function SkinSiteIcon({ site }: { site: SkinSite }) {
  const src = skinSiteFavicon(site.server);
  const fallback = src.replace(/favicon\.png$/, "favicon.ico");
  const [failed, setFailed] = useState("");
  const current = failed === src ? fallback : src;
  if (!src || failed === fallback) {
    return (
      <span
        className="skin-site-icon skin-site-icon--fallback"
        aria-hidden="true"
      >
        {Array.from(site.name.trim())[0]?.toUpperCase() || "?"}
      </span>
    );
  }
  return (
    <img
      key={current}
      className="skin-site-icon"
      src={current}
      alt=""
      width={16}
      height={16}
      referrerPolicy="no-referrer"
      draggable={false}
      onError={() => setFailed(current)}
    />
  );
}
