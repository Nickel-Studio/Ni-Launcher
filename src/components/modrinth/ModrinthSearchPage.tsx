/** 社区检索结果区：整卡进入内部详情。 */
import { Button, LoadingCard } from "../ui";
import { IconRefresh } from "../ui/icons";
import { IconAlert } from "../resources/icons";
import { failureLabel } from "../../lib/failure";
import { ModrinthResultCard } from "./ModrinthResultCard";
import { projectIdentity } from "../community/dependencies";
import type { CommunitySearchView } from "../community/searchView";
import type { ModrinthHit, ModrinthKind } from "./types";

interface Props {
  kind: ModrinthKind;
  /** 父组件收敛好的检索视图（单源或合并）。 */
  view: CommunitySearchView;
  /** 检索中的加载文案（合并模式提示两源并行）。 */
  loadingLabel?: string;
  /** 打开独立详情页（Resources.tsx 状态提升：侧边栏让位给详情页）。 */
  onOpen: (hit: ModrinthHit, kind: ModrinthKind) => void;
}

export function ModrinthSearchPage({
  kind,
  view,
  loadingLabel = "正在检索",
  onOpen,
}: Props) {
  return (
    <div className="cv-engine">
      {view.state === "loading" ? <LoadingCard label={loadingLabel} /> : null}

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
          {view.hits.map((hit) => (
            <ModrinthResultCard
              key={projectIdentity(hit)}
              hit={hit}
              kind={kind}
              onOpen={onOpen}
            />
          ))}
        </ul>
      ) : null}
    </div>
  );
}
