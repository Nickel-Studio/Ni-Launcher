import { useEffect, useRef, useState } from "react";
import { Button, IconButton, Tooltip } from "../ui";
import {
  IconPlus,
  IconPencil,
  IconRefresh,
  IconTrash,
  IconDownload,
  IconClose,
} from "../ui/icons";
import { IconFolder } from "./icons";
import { pathEllipsis, rootDisplayName } from "./instanceVisuals";

export interface VersionRootNavProps {
  roots: string[];
  labels: Record<string, string>;
  /** 启动器自带 / 便携包定义的游戏目录；展示为「默认实例目录」。 */
  defaultDir: string;
  active: string;
  busy: boolean;
  onSelect: (dir: string) => void;
  onAdd: () => void;
  onCreate: () => void;
  onImport: () => void;
  onRemove: (dir: string) => void;
  onDelete: (dir: string) => void;
  onRename: (dir: string) => void;
  onOpen: (dir: string) => void;
  onRefresh: (dir: string) => void;
}

/** 目录身份始终为完整路径；菜单改名只改注册表标签。 */
export function VersionRootNav({
  roots,
  labels,
  defaultDir,
  active,
  busy,
  onSelect,
  onAdd,
  onCreate,
  onImport,
  onRemove,
  onDelete,
  onRename,
  onOpen,
  onRefresh,
}: VersionRootNavProps) {
  const [menu, setMenu] = useState<string | null>(null);
  const nav = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!menu) return;
    const close = (event: PointerEvent) => {
      if (event.target instanceof Node && !nav.current?.contains(event.target))
        setMenu(null);
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenu(null);
    };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", close);
      document.removeEventListener("keydown", key);
    };
  }, [menu]);
  const run = (action: (dir: string) => void, dir: string) => {
    setMenu(null);
    action(dir);
  };
  return (
    <nav className="vs-nav" aria-label="实例目录" ref={nav}>
      <div className="vs-nav__head">
        <div className="vs-nav__title nw-eyebrow">实例目录</div>
      </div>
      <ul className="vs-nav__list">
        {roots.map((dir) => {
          const label = rootDisplayName(dir, labels, defaultDir);
          return (
            <li
              key={dir}
              className="vs-nav__row"
              onContextMenu={(event) => {
                event.preventDefault();
                setMenu(dir);
              }}
            >
              <Tooltip label={dir}>
                <button
                  type="button"
                  className={`vs-nav__item${
                    dir === active ? " is-active" : ""
                  }`}
                  disabled={busy}
                  aria-current={dir === active ? "true" : undefined}
                  onClick={() => {
                    setMenu(null);
                    onSelect(dir);
                  }}
                >
                  <span className="vs-nav__icon">
                    <IconFolder />
                  </span>
                  <span className="vs-nav__name">{label}</span>
                  <span className="vs-nav__path nw-mono">
                    {pathEllipsis(dir, 34)}
                  </span>
                </button>
              </Tooltip>
              <IconButton
                label={`管理目录 ${label}`}
                size="sm"
                className="vs-nav__remove"
                disabled={busy}
                aria-expanded={menu === dir}
                aria-haspopup="menu"
                onClick={() => setMenu(menu === dir ? null : dir)}
              >
                <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden>
                  <circle cx="5" cy="12" r="1.5" />
                  <circle cx="12" cy="12" r="1.5" />
                  <circle cx="19" cy="12" r="1.5" />
                </svg>
              </IconButton>
              {menu === dir ? (
                <div
                  className="vs-nav__menu"
                  role="menu"
                  aria-label={`${label} 目录操作`}
                >
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => run(onRename, dir)}
                  >
                    <IconPencil />
                    重命名
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => run(onOpen, dir)}
                  >
                    <IconFolder />
                    打开目录
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => run(onRefresh, dir)}
                  >
                    <IconRefresh />
                    刷新
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => run(onRemove, dir)}
                  >
                    <IconClose />
                    移出列表
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    className="is-danger"
                    onClick={() => run(onDelete, dir)}
                  >
                    <IconTrash />
                    删除磁盘目录
                  </button>
                </div>
              ) : null}
            </li>
          );
        })}
      </ul>
      {!roots.length ? (
        <p className="vs-nav__empty">
          添加或新建 .minecraft 目录开始管理实例。
        </p>
      ) : null}
      <div className="vs-nav__add">
        <span className="nw-eyebrow">添加或导入</span>
        <Button
          variant="ghost"
          size="sm"
          iconLeft={<IconFolder />}
          disabled={busy}
          onClick={onCreate}
        >
          新建 .minecraft 文件夹
        </Button>
        <Button
          variant="ghost"
          size="sm"
          iconLeft={<IconPlus />}
          disabled={busy}
          onClick={onAdd}
        >
          添加已有文件夹
        </Button>
        <Button
          variant="ghost"
          size="sm"
          iconLeft={<IconDownload />}
          disabled={busy || !active}
          onClick={onImport}
        >
          导入整合包
        </Button>
      </div>
    </nav>
  );
}
