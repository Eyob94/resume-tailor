import { useState, type FormEvent, type ReactNode } from "react";
import { Save } from "lucide-react";
import { errorMessage, type JobSearch, type PostedWithin, type SearchInput, type WorkMode } from "../api";
import { ChipInput } from "./ChipInput";
import { Alert, Button } from "./ui";

export const WORK_MODE_LABELS: Record<WorkMode, string> = { onsite: "On-site", hybrid: "Hybrid", remote: "Remote" };
const POSTED_LABELS: Record<PostedWithin, string> = { day: "Past 24 hours", week: "Past week", month: "Past month" };

function Field({ label, htmlFor, hint, children }: { label: string; htmlFor?: string; hint?: string; children: ReactNode }) {
  return (
    <div>
      <label htmlFor={htmlFor} className="mb-1.5 block text-sm font-medium">
        {label}
      </label>
      {children}
      {hint && <p className="mt-1 text-xs text-zinc-500">{hint}</p>}
    </div>
  );
}

const inputClass =
  "w-full rounded-lg border border-zinc-200 bg-white px-3 py-2 text-sm shadow-sm outline-none placeholder:text-zinc-400 focus:border-zinc-400 focus:ring-2 focus:ring-zinc-900/10";

function validate(input: SearchInput): string | null {
  if (!input.titles.length && !input.keywords.length) return "Add at least one job title or keyword.";
  if (!input.work_modes.length) return "Pick at least one work mode.";
  if (!input.location && input.work_modes.some((m) => m !== "remote")) return "Add a location for on-site or hybrid searches.";
  return null;
}

export function SearchForm({
  initial,
  onSubmit,
  onCancel,
}: {
  initial?: JobSearch;
  onSubmit: (input: SearchInput) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [titles, setTitles] = useState<string[]>(initial?.titles ?? []);
  const [keywords, setKeywords] = useState<string[]>(initial?.keywords ?? []);
  const [location, setLocation] = useState(initial?.location ?? "");
  const [workModes, setWorkModes] = useState<WorkMode[]>(initial?.work_modes ?? ["onsite", "hybrid", "remote"]);
  const [postedWithin, setPostedWithin] = useState<PostedWithin>(initial?.posted_within ?? "week");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const remoteOnly = workModes.length > 0 && workModes.every((m) => m === "remote");

  async function submit(e: FormEvent) {
    e.preventDefault();
    const input: SearchInput = {
      name: name.trim() || undefined,
      titles,
      keywords,
      location: location.trim() || undefined,
      work_modes: workModes,
      posted_within: postedWithin,
    };
    const problem = validate(input);
    if (problem) {
      setError(problem);
      return;
    }
    setError(null);
    setSaving(true);
    try {
      await onSubmit(input);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <form onSubmit={submit} className="space-y-5 p-5">
      {error && <Alert onDismiss={() => setError(null)}>{error}</Alert>}

      <div className="grid gap-5 sm:grid-cols-2">
        <Field label="Job titles" htmlFor="titles" hint="Jobs must contain every word of one title, e.g. “Backend Engineer”.">
          <ChipInput id="titles" values={titles} onChange={setTitles} placeholder="Backend Engineer, Software Engineer" />
        </Field>
        <Field label="Keywords" htmlFor="keywords" hint="At least one must appear in the title or description.">
          <ChipInput id="keywords" values={keywords} onChange={setKeywords} placeholder="Rust, TypeScript" />
        </Field>
      </div>

      <div className="grid gap-5 sm:grid-cols-2">
        <Field label="Work mode">
          <div className="flex flex-wrap gap-2">
            {(Object.keys(WORK_MODE_LABELS) as WorkMode[]).map((mode) => {
              const on = workModes.includes(mode);
              return (
                <button
                  key={mode}
                  type="button"
                  aria-pressed={on}
                  onClick={() => setWorkModes(on ? workModes.filter((m) => m !== mode) : [...workModes, mode])}
                  className={`rounded-lg border px-3 py-2 text-sm font-medium transition-colors ${
                    on ? "border-zinc-900 bg-zinc-900 text-white" : "border-zinc-200 bg-white text-zinc-600 hover:bg-zinc-50"
                  }`}
                >
                  {WORK_MODE_LABELS[mode]}
                </button>
              );
            })}
          </div>
        </Field>
        <Field
          label="Location"
          htmlFor="location"
          hint={remoteOnly ? "Optional for remote-only searches." : "City or country, as you'd type it on LinkedIn."}
        >
          <input id="location" value={location} onChange={(e) => setLocation(e.target.value)} placeholder="United Arab Emirates" className={inputClass} />
        </Field>
      </div>

      <div className="grid gap-5 sm:grid-cols-2">
        <Field label="Posted within" htmlFor="posted">
          <select id="posted" value={postedWithin} onChange={(e) => setPostedWithin(e.target.value as PostedWithin)} className={inputClass}>
            {(Object.keys(POSTED_LABELS) as PostedWithin[]).map((p) => (
              <option key={p} value={p}>
                {POSTED_LABELS[p]}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Name" htmlFor="name" hint="Optional. Defaults to the first title and location.">
          <input id="name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Backend · Dubai" className={inputClass} />
        </Field>
      </div>

      <div className="flex justify-end gap-2 border-t border-zinc-100 pt-4">
        <Button type="button" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" icon={Save} loading={saving}>
          {initial ? "Save changes" : "Save search"}
        </Button>
      </div>
    </form>
  );
}
