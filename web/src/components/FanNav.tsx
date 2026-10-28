import { useEffect, useRef, useState } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { LayoutGrid, X, type LucideIcon } from "lucide-react";

export function FanNav({ items }: { items: { to: string; shortLabel: string; icon: LucideIcon }[] }) {
  const location = useLocation();
  const [open, setOpen] = useState(false);
  const [backdropMounted, setBackdropMounted] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const currentItemRef = useRef<HTMLAnchorElement>(null);

  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReducedMotion(media.matches);
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    if (open) {
      setBackdropMounted(true);
      const frame = window.requestAnimationFrame(() => currentItemRef.current?.focus());
      return () => window.cancelAnimationFrame(frame);
    }
    const timeout = window.setTimeout(() => setBackdropMounted(false), 250);
    return () => window.clearTimeout(timeout);
  }, [open]);

  useEffect(() => {
    setOpen(false);
  }, [location.pathname]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeMenu();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open]);

  function closeMenu() {
    setOpen(false);
    buttonRef.current?.focus();
  }

  const points = [90, 120, 150, 180];
  const openDelays = [0, 40, 80, 120];
  const closeDelays = [120, 80, 40, 0];

  return (
    <>
      {backdropMounted && (
        <button
          type="button"
          aria-label="Close navigation"
          tabIndex={open ? 0 : -1}
          onClick={closeMenu}
          className={`fixed inset-0 z-30 bg-zinc-900/10 backdrop-blur-[1px] transition-opacity duration-200 md:hidden ${open ? "opacity-100" : "opacity-0"}`}
        />
      )}
      <div
        className="fixed right-5 z-40 md:hidden"
        style={{ bottom: "calc(env(safe-area-inset-bottom) + 20px)" }}
      >
        <nav
          id="fan-nav"
          aria-label="Pages"
          aria-hidden={!open}
          className="pointer-events-none absolute inset-0 size-[52px]"
        >
          {items.map(({ to, shortLabel, icon: Icon }, index) => {
            const angle = (points[index] ?? 90) * (Math.PI / 180);
            const x = 136 * Math.cos(angle);
            const y = -136 * Math.sin(angle);
            const delay = reducedMotion ? 0 : open ? openDelays[index] : closeDelays[index];
            const transition = reducedMotion
              ? "opacity 200ms ease-out"
              : open
                ? "transform 420ms cubic-bezier(0.34, 1.56, 0.64, 1), opacity 200ms ease-out"
                : "transform 220ms ease-in, opacity 150ms ease-in";
            const transform = reducedMotion
              ? `translate(${x}px, ${y}px) scale(1)`
              : open
                ? `translate(${x}px, ${y}px) scale(1)`
                : "translate(0, 0) scale(0.3)";

            return (
              <div
                key={to}
                className="absolute right-0.5 top-0.5"
                style={{
                  transform,
                  opacity: open ? 1 : 0,
                  pointerEvents: open ? "auto" : "none",
                  transition,
                  transitionDelay: `${delay}ms`,
                }}
              >
                <NavLink
                  ref={location.pathname.startsWith(to) ? currentItemRef : undefined}
                  to={to}
                  tabIndex={open ? 0 : -1}
                  aria-hidden={!open}
                  onClick={closeMenu}
                  className="flex flex-row-reverse items-center gap-2"
                >
                  {({ isActive }) => (
                    <>
                      <span
                        className={`flex size-12 shrink-0 items-center justify-center rounded-full shadow-md active:scale-90 ${
                          isActive ? "bg-zinc-900 text-white" : "bg-white text-zinc-700 ring-1 ring-zinc-200"
                        }`}
                      >
                        <Icon className="size-5" />
                      </span>
                      <span
                        className={`whitespace-nowrap rounded-md px-2 py-1 text-xs font-medium shadow-sm ${
                          isActive ? "bg-zinc-900 text-white ring-0" : "bg-white text-zinc-700 ring-1 ring-zinc-200"
                        }`}
                        style={{
                          opacity: open ? 1 : 0,
                          transform: reducedMotion || open ? "translateX(0)" : "translateX(4px)",
                          transition: reducedMotion ? "opacity 200ms ease-out" : "opacity 200ms ease-out, transform 220ms ease-out",
                          transitionDelay: `${delay}ms`,
                        }}
                      >
                        {shortLabel}
                      </span>
                    </>
                  )}
                </NavLink>
              </div>
            );
          })}
        </nav>
        <button
          ref={buttonRef}
          type="button"
          aria-label={open ? "Close navigation" : "Open navigation"}
          aria-expanded={open}
          aria-controls="fan-nav"
          onClick={() => (open ? closeMenu() : setOpen(true))}
          className={`relative z-40 flex size-[52px] items-center justify-center rounded-full bg-zinc-900 text-white shadow-lg shadow-zinc-900/25 transition-transform active:scale-95 ${open ? "ring-4 ring-white" : ""}`}
        >
          <LayoutGrid className={`absolute size-6 transition-all duration-300 ${open ? "scale-75 rotate-90 opacity-0" : "scale-100 rotate-0 opacity-100"}`} />
          <X className={`absolute size-6 transition-all duration-300 ${open ? "scale-100 rotate-0 opacity-100" : "scale-75 rotate-90 opacity-0"}`} />
        </button>
      </div>
    </>
  );
}
