import { useState } from "react";
import type { ReactNode } from "react";

/**
 * Image that swaps to a placeholder element when the source is missing or
 * fails to load. The placeholder renders inside a div carrying the same
 * className plus the given placeholder class (for sizing + centering styles).
 */
export function ImageWithFallback({
  src,
  alt,
  className,
  placeholderClassName,
  placeholder,
  loading,
}: {
  src: string | null;
  alt: string;
  className: string;
  placeholderClassName: string;
  placeholder: ReactNode;
  loading?: "lazy";
}) {
  const [broken, setBroken] = useState(false);

  if (!src || broken) {
    return <div className={`${className} ${placeholderClassName}`}>{placeholder}</div>;
  }

  return (
    <img
      className={className}
      src={src}
      alt={alt}
      loading={loading}
      onError={() => setBroken(true)}
    />
  );
}
