import { useCallback, useEffect, useRef, useState } from "react";

export interface ToastState {
  message: string;
  kind: "info" | "error";
}

export function useToast(): {
  toast: ToastState | null;
  showToast: (message: string, kind?: ToastState["kind"]) => void;
} {
  const [toast, setToast] = useState<ToastState | null>(null);
  const timerRef = useRef<number | undefined>(undefined);

  const showToast = useCallback(
    (message: string, kind: ToastState["kind"] = "info") => {
      window.clearTimeout(timerRef.current);
      setToast({ message, kind });
      timerRef.current = window.setTimeout(() => setToast(null), 4000);
    },
    [],
  );

  useEffect(() => () => window.clearTimeout(timerRef.current), []);

  return { toast, showToast };
}

export function Toast({ toast }: { toast: ToastState | null }) {
  if (!toast) return null;
  return <div className={`toast toast-${toast.kind}`}>{toast.message}</div>;
}
