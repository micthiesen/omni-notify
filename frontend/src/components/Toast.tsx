import { useCallback, useEffect, useRef, useState } from "react";
import { Effect } from "effect";
import { forkUiEffect } from "../effect";

export interface ToastState {
  message: string;
  kind: "info" | "error";
}

export function useToast(): {
  toast: ToastState | null;
  showToast: (message: string, kind?: ToastState["kind"]) => void;
} {
  const [toast, setToast] = useState<ToastState | null>(null);
  const cancelTimerRef = useRef<(() => void) | undefined>(undefined);

  const showToast = useCallback(
    (message: string, kind: ToastState["kind"] = "info") => {
      cancelTimerRef.current?.();
      setToast({ message, kind });
      cancelTimerRef.current = forkUiEffect(
        Effect.sleep("4 seconds").pipe(
          Effect.tap(() => Effect.sync(() => setToast(null))),
        ),
      );
    },
    [],
  );

  useEffect(() => () => cancelTimerRef.current?.(), []);

  return { toast, showToast };
}

export function Toast({ toast }: { toast: ToastState | null }) {
  if (!toast) return null;
  return <div className={`toast toast-${toast.kind}`}>{toast.message}</div>;
}
