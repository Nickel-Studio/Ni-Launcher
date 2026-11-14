import type { HTMLAttributes, ReactNode } from "react";
import "./LoadingCard.css";

export type LoadingCardSize = "page" | "inline";
export type LoadingCardState = "loading" | "error";

export interface LoadingCardProps
  extends Omit<HTMLAttributes<HTMLDivElement>, "children"> {
  /** 加载 / 错误文案；省略时按状态使用默认值。 */
  label?: string;
  /** page = 独立页面占位（自带卡片）；inline = 嵌入现有卡片 / 面板。 */
  size?: LoadingCardSize;
  /** error 会让矿镐停摆、矿石出现叉号并整体转红，最后一帧定格。 */
  state?: LoadingCardState;
  /** 错误态下的重试 / 返回等操作。 */
  action?: ReactNode;
}

/**
 * 可见画布：原 160×144 坐标整体上移 30，裁掉顶部空白后为 160×114。
 * 这里直接平移绘制坐标，而不是用 `viewBox="0 30 160 114"` 做负向裁切：
 * `transform-box: view-box` 的参考原点在部分 WebView 会受 viewBox 的 min-x/min-y
 * 影响，同一条 `transform-origin` 可能让旋转支点偏移 30 单位；保持 viewBox 原点
 * 为 (0, 0) 才能让镐子的挥动支点跨平台一致。
 */
const ART_VIEW_WIDTH = 160;
const ART_VIEW_HEIGHT = 114;

/** 侧视线稿；44% 时镐尖落在矿块顶边，裂纹与碎屑随后出现。
 *  坐标已按裁切量整体上移 30：原落点 (104, 86) → (104, 56)。 */
function MiningArt() {
  return (
    <svg
      className="nw-loading__art"
      viewBox={`0 0 ${ART_VIEW_WIDTH} ${ART_VIEW_HEIGHT}`}
      width={ART_VIEW_WIDTH}
      height={ART_VIEW_HEIGHT}
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <path className="nw-loading__ground" d="M74 100h62" />
      <g className="nw-loading__block">
        <path d="M88 56h32l6 6v30l-6 6H88l-6-6V62Z" />
        <path
          className="nw-loading__ore"
          d="m92 71 4-3 4 4-4 4Zm16 13 4-4 5 3-3 5Z"
        />
        <path className="nw-loading__crack" d="m104 56-3 8 7 6-5 9" />
      </g>
      <g className="nw-loading__chips">
        <path d="m96 50-4-4m14 1 2-5m8 12 5-2" />
      </g>
      <g className="nw-loading__pick">
        <path
          className="nw-loading__handle"
          d="m78 35-35 43a3 3 0 0 0 5 4l34-43"
        />
        <path
          className="nw-loading__head"
          d="M56 31Q84 15 104 56L88 41Q76 31 56 31Z"
        />
      </g>
      {/* 错误态才出现：矿石正上方的叉号，紧贴镐尖右侧，离开矿块轮廓。 */}
      <path className="nw-loading__ore-x" d="m110 32 16 16m0-16-16 16" />
    </svg>
  );
}

/** 通用加载卡片：动画在上、文案在下，替代各页面零散的旋转指示器。 */
export function LoadingCard({
  label,
  size = "page",
  state = "loading",
  action,
  className,
  ...rest
}: LoadingCardProps) {
  const cls = [
    "nw-loading",
    `nw-loading--${size}`,
    state === "error" ? "nw-loading--error" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  const text = label ?? (state === "error" ? "加载失败" : "正在加载...");

  return (
    <div
      className={cls}
      role={state === "error" ? "alert" : "status"}
      aria-live={state === "error" ? "assertive" : "polite"}
      aria-atomic="true"
      {...rest}
    >
      <div className="nw-loading__card">
        <div className="nw-loading__clip">
          <MiningArt />
        </div>
        <p className="nw-loading__label">{text}</p>
        {action ? <div className="nw-loading__action">{action}</div> : null}
      </div>
    </div>
  );
}
