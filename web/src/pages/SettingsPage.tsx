import { useEffect, useState, type ReactNode } from "react";
import { errorMessage, jobsApi } from "../api";
import { ServerStatus } from "../components/Layout";
import { Alert, Card, PageHeader } from "../components/ui";
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
  const [scanInterval, setScanInterval] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    jobsApi.getSettings().then(
      (s) => setScanInterval(s.scan_interval_minutes),
      (e) => setError(errorMessage(e)),
    );
  }, []);

  async function changeInterval(minutes: number) {
    const previous = scanInterval;
    setScanInterval(minutes);
    try {
      await jobsApi.updateSettings({ scan_interval_minutes: minutes });
    } catch (e) {
      setScanInterval(previous);
      setError(errorMessage(e));
    }
  }

  return (
    <>
      <PageHeader title="Settings" />
      {error && <Alert onDismiss={() => setError(null)}>{error}</Alert>}
      <Card className="divide-y divide-zinc-100">
        <Row label="Server">
          <ServerStatus />
        </Row>
        <Row label="Job scan interval" hint="How often saved searches are checked on LinkedIn.">
          <select
            aria-label="Job scan interval"
            value={scanInterval ?? ""}
            disabled={scanInterval === null}
            onChange={(e) => changeInterval(Number(e.target.value))}
            className="rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-sm text-zinc-900 shadow-sm"
          >
            {[10, 15, 20, 30, 45, 60].map((m) => (
              <option key={m} value={m}>
                Every {m === 60 ? "hour" : `${m} min`}
              </option>
            ))}
          </select>
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
