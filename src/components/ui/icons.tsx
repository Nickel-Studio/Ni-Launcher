import type { ComponentType, SVGProps } from "react";
import logoUrl from "../../assets/logo.png";

export type IconProps = SVGProps<SVGSVGElement>;

function base(p: IconProps): IconProps {
  return {
    viewBox: "0 0 24 24",
    width: "1em",
    height: "1em",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.75,
    strokeLinecap: "round",
    strokeLinejoin: "round",
    "aria-hidden": true,
    focusable: false,
    ...p,
  };
}

export function IconNickel(p: IconProps) {
  return (
    <svg {...base(p)} stroke="none">
      <image href={logoUrl} width="24" height="24" />
    </svg>
  );
}

/* ---- Navigation ---------------------------------------------------------- */
export function IconHome(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M3 10.4 12 3l9 7.4" />
      <path d="M5.4 9.2V20.6h13.2V9.2" />
      <path d="M9.6 20.6v-6h4.8v6" />
    </svg>
  );
}

export function IconVersions(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3 3 7.5l9 4.5 9-4.5L12 3Z" />
      <path d="M3 12.4 12 16.9l9-4.5" />
      <path d="M3 16.9 12 21.4l9-4.5" />
    </svg>
  );
}

export function IconResources(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M21 8 12 3 3 8v8l9 5 9-5V8Z" />
      <path d="M3 8l9 5 9-5" />
      <path d="M12 13v8" />
    </svg>
  );
}

export function IconSettings(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M4 6.5h6" />
      <path d="M14 6.5h6" />
      <circle cx="12" cy="6.5" r="2.1" />
      <path d="M4 12h10" />
      <path d="M18 12h2" />
      <circle cx="16" cy="12" r="2.1" />
      <path d="M4 17.5h6" />
      <path d="M14 17.5h6" />
      <circle cx="12" cy="17.5" r="2.1" />
    </svg>
  );
}

/* ---- Account strategies -------------------------------------------------- */
export function IconMicrosoft(p: IconProps) {
  return (
    <svg {...base(p)} stroke="none" fill="currentColor">
      <rect x="3.2" y="3.2" width="7.6" height="7.6" rx="1.2" />
      <rect x="13.2" y="3.2" width="7.6" height="7.6" rx="1.2" />
      <rect x="3.2" y="13.2" width="7.6" height="7.6" rx="1.2" />
      <rect x="13.2" y="13.2" width="7.6" height="7.6" rx="1.2" />
    </svg>
  );
}

export function IconSkin(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3.2a8.8 8.8 0 1 0 0 17.6c1.1 0 1.9-.8 1.9-1.8 0-.5-.2-.9-.5-1.2-.3-.3-.5-.7-.5-1.1 0-1 .8-1.8 1.8-1.8h1.4a4.7 4.7 0 0 0 4.7-4.7c0-3.9-4-7-8.8-7Z" />
      <circle cx="7.6" cy="11.4" r="1.15" fill="currentColor" stroke="none" />
      <circle cx="10.4" cy="7.6" r="1.15" fill="currentColor" stroke="none" />
      <circle cx="15.2" cy="8.6" r="1.15" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function IconUser(p: IconProps) {
  return (
    <svg {...base(p)}>
      <circle cx="12" cy="8" r="3.7" />
      <path d="M4.6 20.2a7.4 7.4 0 0 1 14.8 0" />
    </svg>
  );
}

/* ---- Controls / feedback ------------------------------------------------- */
export function IconMinus(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M5 12h14" />
    </svg>
  );
}

export function IconClose(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M6 6l12 12" />
      <path d="M18 6 6 18" />
    </svg>
  );
}

export function IconCheck(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M4.8 12.6 9.4 17.2 19.2 6.8" />
    </svg>
  );
}

export function IconChevronDown(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M6 9.2 12 15.2 18 9.2" />
    </svg>
  );
}

export function IconSpark(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3.2 13.7 8l4.8 1.7L13.7 11.4 12 16.2 10.3 11.4 5.5 9.7 10.3 8 12 3.2Z" />
      <path d="M18.4 14.6l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8.8-2.2Z" />
    </svg>
  );
}

export function IconWrench(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M15.4 3.4a4.6 4.6 0 0 0-6 6L3.2 15.6l2.3 2.3 6.2-6.2a4.6 4.6 0 0 0 6-6l-2.7 2.6-1.9-1.9 2.6-2.7Z" />
      <path d="M6.4 17.6l.9.9" />
    </svg>
  );
}

export function IconRefresh(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M20.4 12a8.4 8.4 0 1 1-2.5-6" />
      <path d="M20.4 4.2v5.2h-5.2" />
    </svg>
  );
}

export function IconPlus(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 5v14" />
      <path d="M5 12h14" />
    </svg>
  );
}

export function IconTrash(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M4 6.6h16" />
      <path d="M9.2 6.6V4.4h5.6v2.2" />
      <path d="M6.4 6.6 7.3 20h9.4l.9-13.4" />
      <path d="M10.4 10v6" />
      <path d="M13.6 10v6" />
    </svg>
  );
}

export function IconShield(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3 19 6v6c0 4.4-3 7.7-7 9-4-1.3-7-4.6-7-9V6l7-3Z" />
      <path d="M9.2 12.2 11.2 14.2 15 10.2" />
    </svg>
  );
}

export function IconDownload(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3.2v11.4" />
      <path d="M7.6 10.4 12 14.8l4.4-4.4" />
      <path d="M4.6 19.6h14.8" />
    </svg>
  );
}

export function IconGlobe(p: IconProps) {
  return (
    <svg {...base(p)}>
      <circle cx="12" cy="12" r="8.4" />
      <path d="M3.6 12h16.8" />
      <path d="M12 3.6c2.6 2.7 2.6 14.1 0 16.8" />
      <path d="M12 3.6c-2.6 2.7-2.6 14.1 0 16.8" />
    </svg>
  );
}

export function IconImage(p: IconProps) {
  return (
    <svg {...base(p)}>
      <rect x="3.4" y="4.6" width="17.2" height="14.8" rx="2.6" />
      <circle cx="8.8" cy="9.6" r="1.6" />
      <path d="M4.4 17.4 9.6 12.2l4.2 4.2 2.4-2.4 3.4 3.4" />
    </svg>
  );
}

export function IconPalette(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M12 3.2a8.8 8.8 0 0 0 0 17.6c1.2 0 2-.8 2-1.9 0-.5-.2-.9-.5-1.2-.3-.3-.5-.6-.5-1.1 0-1 .8-1.8 1.8-1.8h1.5a4.5 4.5 0 0 0 4.5-4.5c0-3.9-3.9-7.1-8.8-7.1Z" />
      <circle cx="7.4" cy="11.2" r="1.15" fill="currentColor" stroke="none" />
      <circle cx="10.2" cy="7.4" r="1.15" fill="currentColor" stroke="none" />
      <circle cx="15" cy="8.4" r="1.15" fill="currentColor" stroke="none" />
    </svg>
  );
}
export function IconLayers(p: IconProps) {
  return <IconVersions {...p} />;
}

export function IconPlay(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path
        d="M7.6 5.2 18.4 12 7.6 18.8V5.2Z"
        fill="currentColor"
        stroke="none"
      />
    </svg>
  );
}

export function IconPencil(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M4.6 19.4h3l9.6-9.6-3-3-9.6 9.6v3Z" />
      <path d="M14.2 6.8l3 3" />
    </svg>
  );
}

export function IconSwap(p: IconProps) {
  return (
    <svg {...base(p)}>
      <path d="M4 9h13.2l-3.2-3.2" />
      <path d="M20 15H6.8l3.2 3.2" />
    </svg>
  );
}

export const ICONS: Record<string, ComponentType<IconProps>> = {
  nickel: IconNickel,
  home: IconHome,
  versions: IconVersions,
  resources: IconResources,
  settings: IconSettings,
  microsoft: IconMicrosoft,
  skin: IconSkin,
  user: IconUser,
  minus: IconMinus,
  close: IconClose,
  check: IconCheck,
  chevronDown: IconChevronDown,
  spark: IconSpark,
  sparkles: IconSpark,
  wrench: IconWrench,
  refresh: IconRefresh,
  plus: IconPlus,
  trash: IconTrash,
  shield: IconShield,
  download: IconDownload,
  globe: IconGlobe,
  image: IconImage,
  palette: IconPalette,
  layers: IconLayers,
  play: IconPlay,
  pencil: IconPencil,
  swap: IconSwap,
};
