import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { IconCheck, IconChevronDown } from "../ui/icons";
const MAX_SUGGESTIONS = 80;
interface Props {
  versions: readonly string[];
  value: string;
  onChange: (value: string) => void;
  emptyText?: string;
  placeholder?: string;
  ariaLabel?: string;
  disabled?: boolean;
  className?: string;
}
function familyOf(version: string): string {
  const parts = version.split(".");
  return parts.length < 2 ? version : `${parts[0]}.${parts[1]}.x`;
}
export function McVersionCombobox({ versions, value, onChange, emptyText = "没有匹配的版本号", placeholder = "全部 (可输入版本)", ariaLabel, disabled = false, className }: Props) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(value);
  const [activeIndex, setActiveIndex] = useState(-1);
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const options = useRef<Array<HTMLButtonElement | null>>([]);
  const committed = useRef(value);
  const listId = useId();
  useEffect(() => { setDraft(value); committed.current = value; }, [value]);
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  const suggestions = useMemo(() => {
    const keyword = draft.trim().toLowerCase();
    const seen = new Set<string>();
    return versions.filter((version) => {
      const key = keyword ? version : familyOf(version);
      if (seen.has(key) || (keyword && !version.toLowerCase().includes(keyword))) return false;
      seen.add(key); return true;
    }).slice(0, MAX_SUGGESTIONS);
  }, [versions, draft]);
  useEffect(() => { setActiveIndex(-1); }, [draft, open]);
  useEffect(() => { if (activeIndex >= 0) options.current[activeIndex]?.scrollIntoView({ block: "nearest" }); }, [activeIndex]);
  const commit = (text: string) => {
    const next = text.trim();
    setOpen(false); setDraft(next);
    if (next !== committed.current) { committed.current = next; onChange(next); }
  };
  // Pointer and keyboard departure share one commit; moving within the control does not.
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node) && (open || rootRef.current?.contains(document.activeElement))) commit(draft);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open, draft, onChange]);
  const onKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (!open || !suggestions.length) return;
      event.preventDefault();
      setActiveIndex((index) => Math.min(suggestions.length - 1, Math.max(0, index + (event.key === "ArrowDown" ? 1 : -1))));
    } else if (event.key === "Enter") {
      event.preventDefault(); commit(open && activeIndex >= 0 ? suggestions[activeIndex] : draft);
    } else if (event.key === "Escape") {
      if (open) event.stopPropagation();
      setOpen(false); setDraft(value); committed.current = value;
    }
  };
  return <div className={["cv-combo", className].filter(Boolean).join(" ")} ref={rootRef} onBlur={(event) => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) commit(draft);
  }}>
    <div className="cv-combo__field" data-open={open}>
      <input ref={inputRef} className="cv-combo__input" type="text" role="combobox" aria-haspopup="listbox" aria-expanded={open} aria-controls={listId} aria-activedescendant={open && activeIndex >= 0 ? `${listId}-${activeIndex}` : undefined} aria-autocomplete="list" aria-label={ariaLabel} autoComplete="off" spellCheck={false} disabled={disabled} placeholder={placeholder} value={draft} onChange={(event) => setDraft(event.target.value)} onKeyDown={onKey} />
      <button type="button" className="cv-combo__toggle" aria-label="展开版本候选" aria-expanded={open} aria-controls={listId} disabled={disabled} onClick={() => { setOpen((v) => !v); inputRef.current?.focus(); }}>
        <IconChevronDown className="cv-combo__chevron" aria-hidden />
      </button>
    </div>
    {open ? <div className="cv-combo__menu" role="listbox" id={listId} aria-label="版本候选">
      {suggestions.map((version, index) => <button id={`${listId}-${index}`} key={version} type="button" role="option" tabIndex={-1} aria-selected={version === value} data-selected={version === value} data-active={index === activeIndex} className="cv-combo__option" ref={(node) => { options.current[index] = node; }} onMouseDown={(event) => event.preventDefault()} onClick={() => commit(version)}>
        <span className="cv-combo__option-version">{version}</span>
        {version === value ? <IconCheck className="cv-combo__option-check" aria-hidden /> : null}
      </button>)}
      {!suggestions.length ? <div className="cv-combo__empty">{emptyText}</div> : null}
    </div> : null}
  </div>;
}
