import { useSlidePill } from "../../lib/useSlidePill";
import { IconImage, IconShieldCheck, IconUser } from "./icons";
import type { AccountType } from "./types";
import "./account.css";

export interface AccountTypeTabsProps {
  value: AccountType;
  onChange: (value: AccountType) => void;
  /** 附加类名（首页用 home__accounts 让分段撑满卡片宽度）。 */
  className?: string;
}

/**
 * 账号形态分段（正版 / 皮肤站 / 离线）—— 与顶栏导航**完全相同**的滑动胶囊动效。
 *
 * 刻意不复用全局 `SegmentedControl`：设置页依赖它「填充块瞬移」的既有语义，
 * 这里需要的是共享胶囊滑动，因此单独实现并复用 `useSlidePill` 的测量逻辑，
 * 保证两处动效曲线 / 时长一致（--t + --ease-out，取自 tokens）。
 */
const TABS: ReadonlyArray<{
  value: AccountType;
  label: string;
  Icon: typeof IconShieldCheck;
}> = [
  { value: "microsoft", label: "正版", Icon: IconShieldCheck },
  { value: "littleskin", label: "皮肤站", Icon: IconImage },
  { value: "offline", label: "离线", Icon: IconUser },
];

export function AccountTypeTabs({ value, onChange, className }: AccountTypeTabsProps) {
  const activeIndex = TABS.findIndex((tab) => tab.value === value);
  const { navRef, registerTab, pill } = useSlidePill<HTMLDivElement, HTMLButtonElement>(
    activeIndex,
  );

  const cls = ["acc-tabs", className].filter(Boolean).join(" ");

  return (
    <div className={cls} role="tablist" aria-label="账号形态" ref={navRef}>
      <span
        className="acc-tabs__pill"
        data-ready={pill.ready}
        style={{ transform: `translateX(${pill.left}px)`, width: pill.width }}
        aria-hidden="true"
      />
      {TABS.map(({ value: option, label, Icon }, index) => (
        <button
          key={option}
          type="button"
          role="tab"
          ref={registerTab(index)}
          className="acc-tabs__item"
          data-active={option === value}
          aria-selected={option === value}
          onClick={() => onChange(option)}
        >
          <Icon />
          <span>{label}</span>
        </button>
      ))}
    </div>
  );
}

export default AccountTypeTabs;
