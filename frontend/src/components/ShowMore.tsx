import { useEffect, useState } from "react";

/**
 * Incrementally reveal a long list: render `pageSize` items up front and grow
 * by another page each click. `resetKey` collapses back to the first page when
 * the underlying view changes (filter, search, entity switch).
 */
export function useShowMore<T>(
  items: T[],
  pageSize: number,
  resetKey?: unknown,
): {
  visible: T[];
  hasMore: boolean;
  remaining: number;
  showMore: () => void;
} {
  const [count, setCount] = useState(pageSize);
  // biome-ignore lint/correctness/useExhaustiveDependencies: resetKey identifies the view
  useEffect(() => {
    setCount(pageSize);
  }, [resetKey, pageSize]);
  return {
    visible: items.slice(0, count),
    hasMore: items.length > count,
    remaining: Math.max(0, items.length - count),
    showMore: () => setCount((current) => current + pageSize),
  };
}

export function ShowMoreButton({
  remaining,
  onClick,
}: {
  remaining: number;
  onClick: () => void;
}) {
  return (
    <button type="button" className="show-more-btn" onClick={onClick}>
      Show More
      <span className="show-more-count">{remaining}</span>
    </button>
  );
}
