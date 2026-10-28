import { useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { AlertCircle, Loader2, type LucideIcon } from "lucide-react";

type Variant = "primary" | "secondary" | "danger";

const variants: Record<Variant, string> = {
  primary: "bg-zinc-900 text-white hover:bg-zinc-800 shadow-sm",
  secondary: "bg-white text-zinc-700 border border-zinc-200 hover:bg-zinc-50 hover:text-zinc-900 shadow-sm",
  danger: "text-red-600 hover:bg-red-50",
};

const buttonBase =
  "inline-flex items-center justify-center gap-2 rounded-lg px-3.5 py-2 text-sm font-medium transition-colors " +
  "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-900 disabled:pointer-events-none disabled:opacity-50";

export function Button({
  variant = "secondary",
  icon: Icon,
  loading,
  children,
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; icon?: LucideIcon; loading?: boolean }) {
  return (
    <button className={`${buttonBase} ${variants[variant]} ${className}`} {...props} disabled={loading || props.disabled}>
      {loading ? <Loader2 className="size-4 shrink-0 animate-spin" /> : Icon && <Icon className="size-4 shrink-0" />}
      {children}
    </button>
  );
}

export function Menu({ trigger, label, items, align = "right" }: {
  trigger: ReactNode;
  label: string;
  items: { label: string; icon?: LucideIcon; onSelect: () => void; danger?: boolean; disabled?: boolean }[];
  align?: "left" | "right";
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onMouseDown = (event: MouseEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onMouseDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    <div ref={root} className="relative inline-block">
      <button
        type="button"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(event) => { event.stopPropagation(); setOpen((value) => !value); }}
        className="inline-flex items-center justify-center border-0 bg-transparent p-0 text-inherit focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-900"
      >
        {trigger}
      </button>
      {open && <div role="menu" className={`absolute z-20 mt-1 min-w-44 rounded-lg border border-zinc-200 bg-white p-1 shadow-lg ${align === "right" ? "right-0" : "left-0"}`}>
        {items.map(({ label: itemLabel, icon: Icon, onSelect, danger, disabled }) => (
          <button
            key={itemLabel}
            type="button"
            role="menuitem"
            disabled={disabled}
            className={`flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-sm ${danger ? "text-red-600 hover:bg-red-50" : "text-zinc-700 hover:bg-zinc-100"} ${disabled ? "pointer-events-none opacity-50" : ""}`}
            onClick={(event) => { event.stopPropagation(); if (disabled) return; onSelect(); setOpen(false); }}
          >
            {Icon && <Icon className="size-4" />}
            {itemLabel}
          </button>
        ))}
      </div>}
    </div>
  );
}

export function ButtonLink({
  to,
  variant = "secondary",
  icon: Icon,
  children,
}: {
  to: string;
  variant?: Variant;
  icon?: LucideIcon;
  children: ReactNode;
}) {
  return (
    <Link to={to} className={`${buttonBase} ${variants[variant]}`}>
      {Icon && <Icon className="size-4 shrink-0" />}
      {children}
    </Link>
  );
}

export function PageHeader({ title, description, actions }: { title: string; description?: string; actions?: ReactNode }) {
  return (
    <div className="mb-4 flex flex-wrap items-end justify-between gap-4 sm:mb-8">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        {description && <p className="mt-1 text-sm text-zinc-500">{description}</p>}
      </div>
      {actions && <div className="flex gap-2">{actions}</div>}
    </div>
  );
}

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`rounded-xl border border-zinc-200 bg-white shadow-sm ${className}`}>{children}</div>;
}

export function Alert({ children, onDismiss }: { children: ReactNode; onDismiss?: () => void }) {
  return (
    <div role="alert" className="mb-6 flex items-start gap-3 rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-800">
      <AlertCircle className="mt-0.5 size-4 shrink-0" />
      <div className="flex-1">{children}</div>
      {onDismiss && (
        <button onClick={onDismiss} className="text-red-600 hover:text-red-800" aria-label="Dismiss">
          ×
        </button>
      )}
    </div>
  );
}

export function EmptyState({
  icon: Icon,
  title,
  children,
  action,
}: {
  icon: LucideIcon;
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <Card className="flex flex-col items-center px-6 py-16 text-center">
      <div className="mb-4 flex size-12 items-center justify-center rounded-full bg-zinc-100">
        <Icon className="size-6 text-zinc-500" />
      </div>
      <h2 className="text-base font-semibold">{title}</h2>
      {children && <p className="mt-1 max-w-sm text-sm text-zinc-500">{children}</p>}
      {action && <div className="mt-6">{action}</div>}
    </Card>
  );
}

export function Skeleton({ className = "" }: { className?: string }) {
  return <div className={`animate-pulse rounded-lg bg-zinc-200/70 ${className}`} />;
}
