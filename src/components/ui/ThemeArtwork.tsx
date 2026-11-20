import { useId } from "react";
import type { Accent, ThemeDecoration } from "../../lib/theme";
import "./theme-artwork.css";

type Props = {
  accent: Accent;
  placement?: "background" | "titlebar";
  decoration?: ThemeDecoration;
  className?: string;
};

function TitlebarMotif({ motif }: { motif: string }) {
  switch (motif) {
    case "nickel":
    case "lattice":
      return <g fill="none" stroke="currentColor" strokeWidth="1">
        <path d="M24 44 52 14 86 24 112 5 143 29 179 12 219 34M24 44 86 24 143 29 219 34M52 14 65 51 112 5 143 29 159 55 219 34" />
        <path d="m179 12 8 31-28 12" />
        <circle cx="86" cy="24" r="3" fill="currentColor" /><circle cx="143" cy="29" r="2" fill="currentColor" />
      </g>;
    case "copper":
    case "ribbons":
      return <g fill="none" stroke="currentColor">
        <path d="M-8 51C55 61 52 4 117 14S174 63 246 20" strokeWidth="8" opacity="0.2" />
        <path d="M-8 45C55 55 52-2 117 8S174 57 246 14M-8 60C55 70 52 13 117 23S174 72 246 29" strokeWidth="1.2" />
        <path d="M178 5v9m-4-5h8M56 47v8m-4-4h8" strokeWidth="1" />
      </g>;
    case "abyss":
      return <g fill="none" stroke="currentColor" strokeWidth="1.2">
        <path d="M-10 35Q35 8 78 35T166 35 254 35M-10 43Q35 16 78 43T166 43 254 43M-10 51Q35 24 78 51T166 51 254 51" />
        <circle cx="167" cy="11" r="2" fill="currentColor" /><path d="M182 14h14m-7-7v14" opacity="0.5" />
      </g>;
    case "paper":
    case "folds":
      return <g stroke="currentColor" strokeWidth="1">
        <path d="M55 0v16m77-16v12m59-12v23" fill="none" />
        <path d="m39 24 16-8 20 9-6 24-24-2Zm78-7 15-5 15 7-5 25-22-5Zm58 14 16-8 17 9-6 22-22-2Z" fill="currentColor" fillOpacity="0.1" />
        <path d="m55 16-10 31m10-31 14 33m63-37-12 27m12-27 10 32m49-21-11 29" fill="none" />
      </g>;
    case "fortyTwo":
    case "orbits":
      return <g fill="none" stroke="currentColor" strokeWidth="1">
        <ellipse cx="112" cy="32" rx="77" ry="20" transform="rotate(-9 112 32)" />
        <ellipse cx="112" cy="32" rx="47" ry="29" transform="rotate(23 112 32)" />
        <path d="m103 21-11 16h15m-2-18v26m10-20q10-12 14-2t-13 17h16" strokeWidth="1.8" />
        <circle cx="186" cy="22" r="3" fill="currentColor" /><circle cx="50" cy="47" r="2" fill="currentColor" />
      </g>;
    default:
      return null;
  }
}

function BackgroundMotif({ motif, glow }: { motif: string; glow: string }) {
  switch (motif) {
    case "nickel":
    case "lattice":
      return <>
        <ellipse cx="1060" cy="110" rx="360" ry="290" fill={`url(#${glow})`} />
        <g fill="currentColor" fillOpacity="0.06" stroke="currentColor" strokeWidth="1.1">
          <path d="m818 50 94-81 124 72-36 146-128-24Zm94-81 88 218m-182-137 218-9m-164 122 40-194" />
          <path d="m1000 187 126-39 132 104-58 139-156-32Zm126-39 74 243m-200-204 200 204m-156-32 214-107" />
          <path d="m64 667 102-78 84 96-40 120-160-38Zm102-78 44 216m-146-138 186 18m-200 82 116-178" />
        </g>
        <g fill="currentColor"><circle cx="1000" cy="187" r="4" /><circle cx="872" cy="163" r="2" /><circle cx="210" cy="805" r="4" /></g>
      </>;
    case "copper":
    case "ribbons":
      return <>
        <ellipse cx="1140" cy="10" rx="450" ry="420" fill={`url(#${glow})`} />
        <g fill="none" stroke="currentColor">
          <path d="M812-20C744 180 1171 32 1116 230S1140 493 1240 414" strokeWidth="78" opacity="0.08" />
          <path d="M788-20C720 180 1147 32 1092 230S1116 493 1216 414M834-20C766 180 1193 32 1138 230S1162 493 1262 414" strokeWidth="1.4" />
          <path d="M-42 604C85 590 61 749 227 776S350 838 380 840" strokeWidth="48" opacity="0.06" />
          <path d="M-42 585C85 571 61 730 227 757S350 819 380 821M-42 622C85 608 61 767 227 794S350 856 380 858" strokeWidth="1.2" />
        </g>
        <path d="m1020 58 5 17 17 5-17 5-5 17-5-17-17-5 17-5Zm153 261 3 10 10 3-10 3-3 10-3-10-10-3 10-3Z" fill="currentColor" opacity="0.6" />
      </>;
    case "abyss":
      return <>
        <ellipse cx="1150" cy="280" rx="340" ry="360" fill={`url(#${glow})`} />
        <g fill="none" stroke="currentColor" strokeWidth="1.1">
          <path d="M807-40C717 109 970 112 1014 215S936 388 1277 462M836-40C746 109 999 112 1043 215S965 388 1306 462M865-40C775 109 1028 112 1072 215S994 388 1335 462M894-40C804 109 1057 112 1101 215S1023 388 1364 462" />
          <path d="M-85 555Q75 641 23 734T262 854M-65 533Q95 619 43 712T282 832M-45 511Q115 597 63 690T302 810" opacity="0.65" />
          <path d="m1140 92 16-9 16 9-16 9Z" /><circle cx="1156" cy="92" r="26" opacity="0.45" />
        </g>
        <g fill="currentColor"><circle cx="945" cy="65" r="2" /><circle cx="1146" cy="223" r="3" /><circle cx="87" cy="644" r="2" /></g>
      </>;
    case "paper":
    case "folds":
      return <>
        <ellipse cx="1080" cy="20" rx="350" ry="370" fill={`url(#${glow})`} />
        <g stroke="currentColor" strokeWidth="1" fill="none">
          <path d="M932-15v99m151-99v49m89-49v176" opacity="0.7" />
          <path d="m881 114 51-30 69 32-18 91-76-9Zm162-57 40-23 47 25-11 63-61-3Zm86 130 43-26 54 28-15 80-66-10Z" fill="currentColor" fillOpacity="0.08" />
          <path d="m932 84-25 114m25-114 51 123m-102-93 120 2m82-82-25 85m25-85 36 88m-76-65 87 2m42 102-27 98m27-98 39 108" />
          <path d="m-40 710 171-96 105 106-81 140Zm0 0 195 150m-24-246 24 246m-195-150 276 10" fill="currentColor" fillOpacity="0.06" />
        </g>
      </>;
    case "fortyTwo":
    case "orbits":
      return <>
        <ellipse cx="1054" cy="180" rx="350" ry="320" fill={`url(#${glow})`} />
        <g fill="none" stroke="currentColor" strokeWidth="1.1">
          <ellipse cx="1080" cy="108" rx="202" ry="124" transform="rotate(-24 1080 108)" />
          <ellipse cx="1080" cy="108" rx="132" ry="195" transform="rotate(32 1080 108)" opacity="0.7" />
          <ellipse cx="1080" cy="108" rx="232" ry="146" transform="rotate(-24 1080 108)" opacity="0.3" />
          <path d="M966 226q-28 55 32 72 30-58-32-72Zm0 0 32 72M94 630q-72 21-41 89 70-24 41-89Zm0 0-41 89" fill="currentColor" fillOpacity="0.06" />
          <ellipse cx="69" cy="747" rx="153" ry="57" transform="rotate(20 69 747)" opacity="0.6" />
          <path d="m1063 69-42 58h56m-7-66v94m27-71q34-36 48-7t-46 65h52" strokeWidth="2" opacity="0.4" />
        </g>
        <g fill="currentColor"><circle cx="886" cy="177" r="5" /><circle cx="1169" cy="-16" r="3" /><circle cx="198" cy="797" r="4" /></g>
      </>;
    default:
      return null;
  }
}

export function ThemeArtwork({ accent, placement = "background", decoration = "lattice", className = "" }: Props) {
  const id = useId().replace(/:/g, "");
  const motif = accent === "custom" ? decoration : accent;
  if (["mono", "emerald", "blue", "violet", "amber", "rose", "none"].includes(motif)) return null;
  const glow = `theme-glow-${id}`;
  return (
    <svg
      className={`ni-theme-artwork ni-theme-artwork--${placement} ${className}`}
      data-motif={motif}
      viewBox={placement === "background" ? "0 0 1200 800" : "0 0 240 64"}
      preserveAspectRatio={placement === "background" ? "xMidYMid slice" : "xMidYMid meet"}
      aria-hidden="true"
      focusable="false"
    >
      {placement === "titlebar" ? <TitlebarMotif motif={motif} /> : <>
        <defs>
          <radialGradient id={glow}>
            <stop offset="0" stopColor="currentColor" stopOpacity="0.4" />
            <stop offset="0.55" stopColor="currentColor" stopOpacity="0.12" />
            <stop offset="1" stopColor="currentColor" stopOpacity="0" />
          </radialGradient>
        </defs>
        <BackgroundMotif motif={motif} glow={glow} />
      </>}
    </svg>
  );
}
