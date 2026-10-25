import { useEffect, useRef, type ReactNode } from "react";

export function Dialog({
  title,
  children,
  actions,
  onClose,
}: {
  title: string;
  children: ReactNode;
  actions: ReactNode;
  onClose: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    panel.current?.querySelector<HTMLElement>("button")?.focus();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-zinc-900/40 p-4 sm:items-center" onClick={onClose}>
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby="dialog-title"
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-lg rounded-xl border border-zinc-200 bg-white p-5 shadow-xl"
      >
        <h2 id="dialog-title" className="text-base font-semibold">
          {title}
        </h2>
        <div className="mt-2 text-sm text-zinc-600">{children}</div>
        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">{actions}</div>
      </div>
    </div>
  );
}
