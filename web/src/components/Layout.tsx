import { useEffect, useState } from "react";
import { NavLink, Outlet } from "react-router-dom";
import { FileText, Search, Settings, Workflow, type LucideIcon } from "lucide-react";
import { api } from "../api";
import { FanNav } from "./FanNav";

const nav: { to: string; label: string; shortLabel: string; icon: LucideIcon; end?: boolean }[] = [
  { to: "/jobs", label: "Jobs", shortLabel: "Jobs", icon: Search },
  { to: "/resume", label: "Master resume", shortLabel: "Resume", icon: FileText },
  { to: "/workflow", label: "Workflow", shortLabel: "Workflow", icon: Workflow },
  { to: "/settings", label: "Settings", shortLabel: "Settings", icon: Settings },
];

function Brand() {
  return (
    <div className="flex items-center gap-2.5">
      <img src="/favicon.svg" alt="" className="size-7" />
      <span className="font-semibold tracking-tight">Resume Tailor</span>
    </div>
  );
}

export function Layout() {
  return (
    <div className="flex min-h-screen flex-col md:flex-row">
      <FanNav items={nav} />

      <aside className="sticky top-0 hidden h-screen w-60 shrink-0 flex-col border-r border-zinc-200 bg-white md:flex">
        <div className="px-5 py-5">
          <Brand />
        </div>
        <nav className="flex flex-col gap-1 px-3">
          {nav.map(({ to, label, icon: Icon, end }) => (
            <NavLink
              key={to}
              to={to}
              end={end}
              className={({ isActive }) =>
                `flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm font-medium transition-colors ${
                  isActive ? "bg-zinc-100 text-zinc-900" : "text-zinc-500 hover:bg-zinc-50 hover:text-zinc-900"
                }`
              }
            >
              <Icon className="size-4" />
              {label}
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto px-5 py-4">
          <ServerStatus />
        </div>
      </aside>
      <main className="min-w-0 flex-1 px-4 pt-[calc(env(safe-area-inset-top)+1.5rem)] pb-28 sm:px-8 md:py-8 md:pb-0 lg:px-12">
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
