/**
 * 可输入的 MC 版本下拉（自绘 combobox）。
 *
 * 视觉对齐 ui/Select（填充式圆角、无 1px 描边，accent 只用于焦点环与选中）：
 * 输入框聚焦展开建议列表——未输入时只列「主要版本」（每个版本族最新的一个），
 * 输入后按版本 id 全量匹配过滤；点 field 任意处（含右侧箭头）都会展开列表，
 * 键盘 ↑/↓ 选择、Enter 确认（列表高亮项优先，否则提交输入框原文）、
 * Escape / 点击外部 / 失焦关闭。
 * 空值 = 「全部」（由 placeholder 表达）。
 */
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { IconCheck, IconChevronDown } from "../ui/icons";

/** 建议列表一次最多展示的版本条数（输入后按匹配自然收窄）。 */
const MAX_SUGGESTIONS = 80;

interface Props {
  /** 全量版本 id（来自 `list_remote_versions` 的 versions[].id，时间倒序）。 */
  versions: readonly string[];
  /** 当前选中版本；空串 = 「全部」。 */
  value: string;
  onChange: (value: string) => void;
  /** 建议列表为空时的占位文案（区分「清单不可用」与「没有匹配」）。 */
  emptyText?: string;
  placeholder?: string;
  ariaLabel?: string;
  disabled?: boolean;
  className?: string;
}

/**
 * 版本 id → 版本族（用于「主要版本」去重）：
 * `1.21.4` / `1.21` → `1.21.x`；单段 id（如远古版本）自成一组。
 */
function familyOf(version: string): string {
  const parts = version.split(".");
  if (parts.length < 2) return version;
  return `${parts[0]}.${parts[1]}.x`;
}

export function McVersionCombobox({
  versions,
  value,
  onChange,
  emptyText = "没有匹配的版本号",
  placeholder = "全部 (可输入版本)",
  ariaLabel,
  disabled = false,
  className,
}: Props) {
  const [open, setOpen] = useState(false);
  /** 输入框草稿；选中值变化（含实例识别回填）时同步回显。 */
  const [draft, setDraft] = useState(value);
  const [activeIndex, setActiveIndex] = useState(-1);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => {
    setDraft(value);
  }, [value]);

  // 建议列表：未输入时按版本族去重只列主要版本；输入后全版本匹配过滤。
  const suggestions = useMemo(() => {
    const keyword = draft.trim().toLowerCase();
    if (!keyword) {
      const seen = new Set<string>();
      const majors: string[] = [];
      for (const version of versions) {
        const family = familyOf(version);
        if (seen.has(family)) continue;
        seen.add(family);
        majors.push(version);
        if (majors.length >= MAX_SUGGESTIONS) break;
      }
      return majors;
    }
    return versions
      .filter((version) => version.toLowerCase().includes(keyword))
      .slice(0, MAX_SUGGESTIONS);
  }, [draft, versions]);

  // 打开或草稿变化时重置键盘高亮。
  useEffect(() => {
    setActiveIndex(-1);
  }, [draft, open]);

  // 键盘高亮项滚入可视区。
  useEffect(() => {
    if (activeIndex < 0) return;
    optionRefs.current[activeIndex]?.scrollIntoView({ block: "nearest" });
  }, [activeIndex]);

  // 点击外部关闭并回显当前选中值。
  useEffect(() => {
    if (!open) return;
    const onDocMouseDown = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
        setDraft(value);
      }
    };
    document.addEventListener("mousedown", onDocMouseDown);
    return () => {
      document.removeEventListener("mousedown", onDocMouseDown);
    };
  }, [open, value]);

  const commit = (next: string) => {
    setOpen(false);
    setDraft(next);
    onChange(next);
  };

  /** 提交输入框原文（Enter 无高亮项 / 失焦时）；空串 = 回到「全部」。 */
  const commitDraft = () => {
    const next = draft.trim();
    if (next === value) {
      setOpen(false);
      setDraft(next);
      return;
    }
    commit(next);
  };

  // 点 field 任意处（含右侧箭头）都展开菜单：点在输入框上时保留原生
  // mousedown（光标定位依赖它），点在 padding / 箭头上时接管并聚焦输入框。
  const onFieldMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (disabled) return;
    setOpen(true);
    if (event.target === inputRef.current) return;
    event.preventDefault();
    inputRef.current?.focus();
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!open || suggestions.length === 0) {
        setOpen(true);
        return;
      }
      setActiveIndex((index) => {
        const delta = event.key === "ArrowDown" ? 1 : -1;
        return Math.min(suggestions.length - 1, Math.max(0, index + delta));
      });
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      const picked = open ? suggestions[activeIndex] : undefined;
      commit(picked ?? draft.trim());
      return;
    }
    if (event.key === "Escape") {
      if (open) {
        event.stopPropagation();
        setOpen(false);
        setDraft(value);
      }
      return;
    }
    if (event.key === "Tab") {
      setOpen(false);
      setDraft(value);
    }
  };

  const cls = ["cv-combo", className].filter(Boolean).join(" ");

  return (
    <div className={cls} ref={rootRef}>
      <div
        className="cv-combo__field"
        data-open={open}
        onMouseDown={onFieldMouseDown}
      >
        <input
          ref={inputRef}
          className="cv-combo__input"
          type="text"
          role="combobox"
          aria-haspopup="listbox"
          aria-expanded={open}
          aria-autocomplete="list"
          aria-label={ariaLabel}
          autoComplete="off"
          spellCheck={false}
          disabled={disabled}
          placeholder={placeholder}
          value={draft}
          onFocus={() => setOpen(true)}
          onChange={(event) => {
            setDraft(event.target.value);
            setOpen(true);
          }}
          onBlur={commitDraft}
          onKeyDown={onKeyDown}
        />
        <IconChevronDown className="cv-combo__chevron" aria-hidden />
      </div>

      {open ? (
        <div className="cv-combo__menu" role="listbox">
          {suggestions.map((version, index) => (
            <button
              key={version}
              type="button"
              role="option"
              aria-selected={version === value}
              data-selected={version === value}
              data-active={index === activeIndex}
              className="cv-combo__option"
              ref={(node) => {
                optionRefs.current[index] = node;
              }}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => commit(version)}
            >
              <span className="cv-combo__option-version">{version}</span>
              {version === value ? (
                <IconCheck className="cv-combo__option-check" aria-hidden />
              ) : null}
            </button>
          ))}
          {suggestions.length === 0 ? (
            <div className="cv-combo__empty">{emptyText}</div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
