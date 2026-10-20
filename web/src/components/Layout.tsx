import { useEffect, useState } from "react";
import { NavLink, Outlet } from "react-router-dom";
import { Briefcase, FileText, Settings, Sparkles, type LucideIcon } from "lucide-react";
import { api } from "../api";

const nav: { to: string; label: string; icon: LucideIcon; end?: boolean }[] = [
  { to: "/", label: "Applications", icon: Briefcase, end: true },
  { to: "/new", label: "New application", icon: Sparkles },
  { to: "/resume", label: "Master resume", icon: FileText },
  { to: "/settings", label: "Settings", icon: Settings },
];

export function Layout() {
  return (
    <div className="flex min-h-screen flex-col md:flex-row">
      <aside className="flex shrink-0 flex-col border-b border-zinc-200 bg-white md:sticky md:top-0 md:h-screen md:w-60 md:border-r md:border-b-0">
        <div className="flex items-center gap-2.5 px-5 py-4 md:py-5">
          <img src="/favicon.svg" alt="" className="size-7" />
          <span className="font-semibold tracking-tight">Resume Tailor</span>
        </div>
        <nav className="flex gap-1 overflow-x-auto px-3 pb-3 md:flex-col md:pb-0">
          {nav.map(({ to, label, icon: Icon, end }) => (
            <NavLink
              key={to}
              to={to}
              end={end}
              className={({ isActive }) =>
                `flex shrink-0 items-center gap-2.5 rounded-lg px-3 py-2 text-sm font-medium transition-colors ${
                  isActive ? "bg-zinc-100 text-zinc-900" : "text-zinc-500 hover:bg-zinc-50 hover:text-zinc-900"
                }`
              }
            >
              <Icon className="size-4" />
              {label}
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto hidden px-5 py-4 md:block">
          <ServerStatus />
        </div>
      </aside>
      <main className="flex-1 px-4 py-8 sm:px-8 lg:px-12">
        <div className="mx-auto max-w-5xl">
          <Outlet />
        </div>
      </main>
    </div>
  );
}

export function ServerStatus() {
  const [online, setOnline] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    const check = () =>
      api.health().then(
        () => !cancelled && setOnline(true),
        () => !cancelled && setOnline(false),
      );
    void check();
    const id = setInterval(check, 15_000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  const [dot, label] =
    online === null ? ["bg-zinc-300", "Checking server…"] : online ? ["bg-emerald-500", "Server connected"] : ["bg-red-500", "Server unreachable"];

  return (
    <div className="flex items-center gap-2 text-xs text-zinc-500">
      <span className={`size-2 rounded-full ${dot}`} />
      {label}
    </div>
  );
}
