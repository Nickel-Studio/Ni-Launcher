/** 社区检索结果区：整卡进入内部详情。 */
import { Button, LoadingCard } from "../ui";
import { IconRefresh } from "../ui/icons";
import { IconAlert } from "../resources/icons";
import { failureLabel } from "../../lib/failure";
import { CurseforgeResultCard } from "./CurseforgeResultCard";
import type { CommunitySearchView } from "../community/searchView";
import type { CurseforgeKind } from "./types";
import type { CurseforgeHit } from "../modrinth/types";
import { withProjectId } from "../modrinth/parse";

interface Props {
  kind: CurseforgeKind;
  /** 父组件收敛好的检索视图（显式选中 CurseForge 的单源数据）。 */
  view: CommunitySearchView;
  /** 打开独立详情页（Resources.tsx 状态提升：侧边栏让位给详情页）。 */
  onOpen: (hit: CurseforgeHit, kind: CurseforgeKind) => void;
}

export function CurseforgeSearchPage({ kind, view, onOpen }: Props) {
  return (
    <div className="cv-engine">
      {view.state === "loading" ? <LoadingCard label="正在检索" /> : null}

      {view.state === "error" ? (
        <LoadingCard
          state="error"
          label={failureLabel(view.message)}
          action={
            <Button
              variant="secondary"
              size="sm"
              iconLeft={<IconRefresh />}
              onClick={view.reload}
            >
              重试
            </Button>
          }
        />
      ) : null}

      {view.state === "blocked" ? (
        <div className="mr-page__notice" role="alert">
          <IconAlert aria-hidden />
          <p>{view.message}</p>
          <Button
            variant="secondary"
            size="sm"
            iconLeft={<IconRefresh />}
            onClick={view.reload}
          >
            重试
          </Button>
        </div>
      ) : null}

      {view.state === "empty" ? (
        <div className="mr-page__notice">
          <p>{view.message}</p>
        </div>
      ) : null}

      {view.hits.length > 0 ? (
        <ul className="mr-list" role="list">
          {view.hits.map((raw) => {
            // 后端已保证 slug 命中都带数字项目 id；缺 id 的脏数据整卡丢弃
            // （与 normalizeSearch 丢脏行同一策略），不渲染半可用的卡片。
            const hit = withProjectId(raw);
            if (!hit) return null;
            return (
              <CurseforgeResultCard
                key={`${hit.source}:${hit.slug}`}
                hit={hit}
                kind={kind}
                onOpen={onOpen}
              />
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}
