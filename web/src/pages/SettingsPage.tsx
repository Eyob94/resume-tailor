import type { ReactNode } from "react";
import { ServerStatus } from "../components/Layout";
import { Card, PageHeader } from "../components/ui";
import { useDevMode } from "../devMode";

function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1 px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
      <div>
        <span className="text-sm font-medium">{label}</span>
        {hint && <p className="mt-0.5 text-xs text-zinc-500">{hint}</p>}
      </div>
      <span className="text-sm text-zinc-500">{children}</span>
    </div>
  );
}

function Switch({ checked, onChange, label }: { checked: boolean; onChange: (on: boolean) => void; label: string }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-900 ${
        checked ? "bg-zinc-900" : "bg-zinc-300"
      }`}
    >
      <span className={`inline-block size-5 rounded-full bg-white shadow transition-transform ${checked ? "translate-x-5.5" : "translate-x-0.5"}`} />
    </button>
  );
}

export function SettingsPage() {
  const [devMode, setDevMode] = useDevMode();

  return (
    <>
      <PageHeader title="Settings" />
      <Card className="divide-y divide-zinc-100">
        <Row label="Server">
          <ServerStatus />
        </Row>
        <Row label="AI backend">Not configured yet</Row>
        <Row label="Data">
          Stored locally in <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-xs">./data</code>
        </Row>
        <Row label="Developer mode" hint="Show raw JSON views for inspecting what the app stores.">
          <Switch checked={devMode} onChange={setDevMode} label="Developer mode" />
        </Row>
      </Card>
    </>
  );
}
