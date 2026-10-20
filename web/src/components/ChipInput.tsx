import { useState } from "react";
import { X } from "lucide-react";

/** Text input that turns entries into removable chips on Enter or comma. */
export function ChipInput({
  id,
  values,
  onChange,
  placeholder,
}: {
  id: string;
  values: string[];
  onChange: (values: string[]) => void;
  placeholder?: string;
}) {
  const [draft, setDraft] = useState("");

  function commit(raw: string) {
    const additions = raw
      .split(",")
      .map((v) => v.trim())
      .filter((v) => v && !values.some((existing) => existing.toLowerCase() === v.toLowerCase()));
    if (additions.length) onChange([...values, ...additions]);
    setDraft("");
  }

  return (
    <div className="flex min-h-10 flex-wrap items-center gap-1.5 rounded-lg border border-zinc-200 bg-white px-2 py-1.5 shadow-sm focus-within:border-zinc-400 focus-within:ring-2 focus-within:ring-zinc-900/10">
      {values.map((value) => (
        <span key={value} className="inline-flex items-center gap-1 rounded-md bg-zinc-100 py-0.5 pr-1 pl-2 text-sm text-zinc-800">
          {value}
          <button
            type="button"
            onClick={() => onChange(values.filter((v) => v !== value))}
            className="rounded p-0.5 text-zinc-400 hover:bg-zinc-200 hover:text-zinc-700"
            aria-label={`Remove ${value}`}
          >
            <X className="size-3" />
          </button>
        </span>
      ))}
      <input
        id={id}
        value={draft}
        onChange={(e) => (e.target.value.includes(",") ? commit(e.target.value) : setDraft(e.target.value))}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit(draft);
          } else if (e.key === "Backspace" && !draft && values.length) {
            onChange(values.slice(0, -1));
          }
        }}
        onBlur={() => draft && commit(draft)}
        placeholder={values.length ? "" : placeholder}
        className="min-w-32 flex-1 bg-transparent px-1 py-0.5 text-sm outline-none placeholder:text-zinc-400"
      />
    </div>
  );
}
