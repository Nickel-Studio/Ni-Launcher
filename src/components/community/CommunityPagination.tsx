import { IconButton } from "../ui";

export function CommunityPagination({
  page,
  hasMore,
  onPrevious,
  onNext,
}: {
  page: number;
  hasMore: boolean;
  onPrevious: () => void;
  onNext: () => void;
}) {
  return (
    <nav className="cv-pager" aria-label="检索结果分页">
      <IconButton label="上一页" disabled={page <= 1} onClick={onPrevious}>
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.75"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="m15 5-7 7 7 7" />
        </svg>
      </IconButton>
      <span
        className="cv-pager__page"
        aria-current="page"
        aria-label={`第 ${page} 页`}
        aria-live="polite"
      >
        {page}
      </span>
      <IconButton label="下一页" disabled={!hasMore} onClick={onNext}>
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.75"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="m9 5 7 7-7 7" />
        </svg>
      </IconButton>
    </nav>
  );
}
