import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";
import { api, errorMessage, jobsApi, profileApi, type AiStatus, type Settings } from "../api";
import { ServerStatus } from "../components/Layout";
import { Alert, Button, Card, PageHeader } from "../components/ui";
import { useDevMode } from "../devMode";
import { getKey, setKey } from "../key";
import { Link } from "react-router-dom";

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

const SCAN_INTERVALS: [keyof Settings, string][] = [
  ["scan_interval_minutes", "LinkedIn"],
  ["indeed_scan_interval_minutes", "Indeed"],
];

function maskKey(key: string): string {
  const parts = key.split("-");
  return [parts[0], ...parts.slice(1, -1).map(() => "••••"), parts[parts.length - 1]].join("-");
}

function KeySection() {
  const [key, setStoredKey] = useState<string | null>(() => getKey());
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);
  const [otherKey, setOtherKey] = useState("");
  const [switching, setSwitching] = useState(false);
  const [creating, setCreating] = useState(false);
  const [keyError, setKeyError] = useState<string | null>(null);

  function copyKey() {
    if (!key) return;
    navigator.clipboard.writeText(key).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      },
      () => undefined,
    );
  }

  async function switchAccount(e: FormEvent) {
    e.preventDefault();
    if (!window.confirm("This device will switch to that account. Save your current key first if you need it.")) return;
    setSwitching(true);
    setKeyError(null);
    try {
      const { key: switched } = await api.keyLogin(otherKey);
      setKey(switched);
      window.location.assign("/jobs");
    } catch (e) {
      setKeyError(errorMessage(e));
      setSwitching(false);
    }
  }

  async function createNewKey() {
    if (!window.confirm("This replaces your old key. Other devices stay signed in.")) return;
    setCreating(true);
    setKeyError(null);
    try {
      const { key: created } = await api.rotateKey();
      setKey(created);
      setStoredKey(created);
    } catch (e) {
      setKeyError(errorMessage(e));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="flex flex-col gap-3 px-5 py-4">
      <div>
        <span className="text-sm font-medium">Your key</span>
        <p className="mt-0.5 text-xs text-zinc-500">Paste this key on another device to open this account.</p>
      </div>
      {key ? (
        <div className="flex flex-wrap items-center gap-2">
          <code className="min-w-0 flex-1 break-all rounded bg-zinc-100 px-2.5 py-1.5 font-mono text-sm text-zinc-900">{revealed ? key : maskKey(key)}</code>
          <Button onClick={() => setRevealed(!revealed)}>{revealed ? "Hide" : "Show"}</Button>
          <Button icon={copied ? Check : Copy} onClick={copyKey}>
            {copied ? "Copied" : "Copy"}
          </Button>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-3">
          <span className="text-sm text-zinc-500">This device doesn't have the key saved.</span>
          <Button variant="primary" loading={creating} onClick={createNewKey}>
            Create a new key
          </Button>
        </div>
      )}
      <div className="flex flex-col gap-2 border-t border-zinc-100 pt-3">
        <span className="text-sm font-medium">Use a key from another device</span>
        <form onSubmit={switchAccount} className="flex flex-col gap-2 sm:flex-row">
          <input
            aria-label="Key from another device"
            value={otherKey}
            onChange={(e) => setOtherKey(e.target.value)}
            placeholder="RT-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX"
            className="min-w-0 flex-1 rounded-lg border border-zinc-200 bg-white px-3 py-1.5 font-mono text-sm text-zinc-900 shadow-sm"
          />
          <Button type="submit" variant="primary" loading={switching} disabled={!otherKey.trim()}>
            Switch
          </Button>
        </form>
        {keyError && (
          <p role="alert" className="text-xs text-red-600">
            {keyError}
          </p>
        )}
      </div>
    </div>
  );
}

export function SettingsPage() {
  const [devMode, setDevMode] = useDevMode();
  const [intervals, setIntervals] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [ai, setAi] = useState<AiStatus | null>(null);
  const [ollamaUrl, setOllamaUrl] = useState("");

  useEffect(() => {
    jobsApi.getSettings().then(
      (s) => setIntervals(s),
      (e) => setError(errorMessage(e)),
    );
    profileApi.aiStatus().then(
      (status) => {
        setAi(status);
        setOllamaUrl(status.ollama_url);
      },
      (e) => setError(errorMessage(e)),
    );
  }, []);

  const [savingUrl, setSavingUrl] = useState(false);

  async function saveOllamaUrl(e: FormEvent) {
    e.preventDefault();
    setSavingUrl(true);
    try {
      const status = await profileApi.updateAiSettings({ ollama_url: ollamaUrl });
      setAi(status);
      setOllamaUrl(status.ollama_url);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSavingUrl(false);
    }
  }

  async function changeInterval(key: keyof Settings, minutes: number) {
    if (!intervals) return;
    const previous = intervals;
    const next = { ...intervals, [key]: minutes };
    setIntervals(next);
    try {
      setIntervals(await jobsApi.updateSettings(next));
    } catch (e) {
      setIntervals(previous);
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
        {SCAN_INTERVALS.map(([key, site]) => (
          <Row key={key} label={`${site} scan interval`} hint={`How often enabled preferences are checked on ${site}.`}>
            <select
              aria-label={`${site} scan interval`}
              value={intervals?.[key] ?? ""}
              disabled={intervals === null}
              onChange={(e) => changeInterval(key, Number(e.target.value))}
              className="rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-sm text-zinc-900 shadow-sm"
            >
              {[1, 5, 10, 15, 20, 30, 45, 60].map((m) => (
                <option key={m} value={m}>
                  Every {m === 60 ? "hour" : `${m} min`}
                </option>
              ))}
            </select>
          </Row>
        ))}
        <Row label="Ollama" hint="AI steps run locally, for free.">
          {ai === null ? (
            "Checking…"
          ) : (
            <span className="inline-flex items-center gap-1.5 text-xs">
              <span className={`size-2 rounded-full ${ai.ollama.reachable ? "bg-emerald-500" : "bg-red-500"}`} />
              {ai.ollama.reachable ? "Ollama running" : "Ollama not running"}
            </span>
          )}
        </Row>
        <Row label="AI models and prompts">
          <Link to="/workflow" className="text-zinc-900 underline underline-offset-2">Models and prompts are on the Workflow tab.</Link>
        </Row>
        <Row label="Ollama URL" hint="Where the Ollama server runs. Change it if Ollama is on another machine.">
          <form onSubmit={saveOllamaUrl} className="flex w-full items-center gap-2 sm:w-auto">
            <input
              id="ollama-url"
              aria-label="Ollama URL"
              type="url"
              value={ollamaUrl}
              onChange={(e) => setOllamaUrl(e.target.value)}
              placeholder="http://127.0.0.1:11434"
              className="min-w-0 flex-1 rounded-lg border border-zinc-200 bg-white px-3 py-1.5 text-sm text-zinc-900 shadow-sm sm:w-64 sm:flex-none"
            />
            <Button type="submit" variant="primary" loading={savingUrl} disabled={!ai || ollamaUrl.trim() === ai.ollama_url}>
              Save
            </Button>
          </form>
        </Row>
        <Row label="Data">
          Stored locally in <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-xs">./data</code>
        </Row>
        <Row label="Developer mode" hint="Show raw JSON views for inspecting what the app stores.">
          <Switch checked={devMode} onChange={setDevMode} label="Developer mode" />
        </Row>
        <KeySection />
      </Card>
    </>
  );
}
