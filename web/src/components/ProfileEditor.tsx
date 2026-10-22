import { useState, type ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Plus, Save, X } from "lucide-react";
import { errorMessage, type Bullet, type ProfileRecord, type ResumeProfile } from "../api";
import { ChipInput } from "./ChipInput";
import { Alert, Button } from "./ui";

const input =
  "w-full rounded-lg border border-zinc-200 bg-white px-3 py-2 text-sm shadow-sm outline-none placeholder:text-zinc-400 focus:border-zinc-400 focus:ring-2 focus:ring-zinc-900/10";

function Field({ label, children, className = "" }: { label: string; children: ReactNode; className?: string }) {
  return (
    <label className={`grid gap-1.5 ${className}`}>
      <span className="text-xs font-medium text-zinc-500">{label}</span>
      {children}
    </label>
  );
}

function Text({ value, onChange, placeholder }: { value: string | null; onChange: (v: string | null) => void; placeholder?: string }) {
  return <input className={input} value={value ?? ""} placeholder={placeholder} onChange={(e) => onChange(e.target.value || null)} />;
}

function Section({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section className="space-y-3 border-t border-zinc-100 px-5 py-5 first:border-t-0">
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-sm font-semibold">{title}</h3>
        {action}
      </div>
      {children}
    </section>
  );
}

function RemoveButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} aria-label={label} className="rounded-md p-1.5 text-zinc-400 hover:bg-red-50 hover:text-red-600">
      <X className="size-4" />
    </button>
  );
}

function AddButton({ children, onClick }: { children: ReactNode; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="inline-flex items-center gap-1 text-sm font-medium text-zinc-600 hover:text-zinc-900">
      <Plus className="size-4" />
      {children}
    </button>
  );
}

function Bullets({
  bullets,
  flagged,
  onChange,
}: {
  bullets: Bullet[];
  flagged: Set<string>;
  onChange: (bullets: Bullet[]) => void;
}) {
  return (
    <div className="space-y-2">
      {bullets.map((b, i) => (
        <div key={b.id || `new-${i}`} className="flex items-start gap-2">
          <span className="mt-2.5 text-zinc-300">•</span>
          <div className="min-w-0 flex-1">
            <textarea
              rows={2}
              className={`${input} resize-y ${flagged.has(b.id) ? "border-amber-300 bg-amber-50/50" : ""}`}
              value={b.text}
              onChange={(e) => onChange(bullets.map((x, j) => (j === i ? { ...x, text: e.target.value } : x)))}
            />
            {flagged.has(b.id) && (
              <p className="mt-1 flex items-center gap-1 text-xs text-amber-700">
                <AlertTriangle className="size-3.5" />
                Not found word for word in your resume. Check the AI didn't reword it.
              </p>
            )}
          </div>
          <RemoveButton label="Remove bullet" onClick={() => onChange(bullets.filter((_, j) => j !== i))} />
        </div>
      ))}
      <AddButton onClick={() => onChange([...bullets, { id: "", text: "" }])}>Add bullet</AddButton>
    </div>
  );
}

export function ProfileEditor({
  record,
  onSave,
  onConfirm,
}: {
  record: ProfileRecord & { profile: ResumeProfile };
  onSave: (profile: ResumeProfile) => Promise<void>;
  onConfirm: () => Promise<void>;
}) {
  const [draft, setDraft] = useState<ResumeProfile>(record.profile);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const flagged = new Set(record.warnings.map((w) => w.item_id));

  function update(change: (p: ResumeProfile) => void) {
    const next = structuredClone(draft);
    change(next);
    setDraft(next);
    setDirty(true);
  }

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const save = () =>
    run(async () => {
      await onSave(draft);
      setDirty(false);
    });

  const c = draft.contact;

  return (
    <div>
      {error && (
        <div className="px-5 pt-5">
          <Alert onDismiss={() => setError(null)}>{error}</Alert>
        </div>
      )}

      <Section title="About you">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Name">
            <Text value={c.name} onChange={(v) => update((p) => void (p.contact.name = v ?? ""))} />
          </Field>
          <Field label="Location">
            <Text value={c.location} onChange={(v) => update((p) => void (p.contact.location = v))} />
          </Field>
          <Field label="Email">
            <Text value={c.email} onChange={(v) => update((p) => void (p.contact.email = v))} />
          </Field>
          <Field label="Phone">
            <Text value={c.phone} onChange={(v) => update((p) => void (p.contact.phone = v))} />
          </Field>
        </div>
        <Field label="Links">
          <ChipInput id="links" values={c.links} onChange={(v) => update((p) => void (p.contact.links = v))} placeholder="linkedin.com/in/…" />
        </Field>
        <Field label="One-line profile (written by the AI)">
          <Text value={draft.headline} onChange={(v) => update((p) => void (p.headline = v))} />
        </Field>
        <Field label="Summary">
          <textarea
            rows={3}
            className={`${input} resize-y`}
            value={draft.summary ?? ""}
            onChange={(e) => update((p) => void (p.summary = e.target.value || null))}
          />
        </Field>
      </Section>

      <Section
        title="Experience"
        action={
          <AddButton
            onClick={() =>
              update((p) => p.experience.push({ id: "", company: "", title: "", location: null, start: null, end: null, bullets: [] }))
            }
          >
            Add job
          </AddButton>
        }
      >
        {draft.experience.map((job, i) => (
          <div key={job.id || `new-${i}`} className="space-y-3 rounded-lg border border-zinc-200 p-4">
            <div className="flex items-start gap-2">
              <div className="grid min-w-0 flex-1 gap-3 sm:grid-cols-2">
                <Field label="Title">
                  <Text value={job.title} onChange={(v) => update((p) => void (p.experience[i].title = v ?? ""))} />
                </Field>
                <Field label="Company">
                  <Text value={job.company} onChange={(v) => update((p) => void (p.experience[i].company = v ?? ""))} />
                </Field>
                <Field label="Start">
                  <Text value={job.start} onChange={(v) => update((p) => void (p.experience[i].start = v))} placeholder="Mar 2021" />
                </Field>
                <Field label="End">
                  <Text value={job.end} onChange={(v) => update((p) => void (p.experience[i].end = v))} placeholder="Present" />
                </Field>
                <Field label="Location" className="sm:col-span-2">
                  <Text value={job.location} onChange={(v) => update((p) => void (p.experience[i].location = v))} />
                </Field>
              </div>
              <RemoveButton label="Remove job" onClick={() => update((p) => void p.experience.splice(i, 1))} />
            </div>
            <Bullets bullets={job.bullets} flagged={flagged} onChange={(b) => update((p) => void (p.experience[i].bullets = b))} />
          </div>
        ))}
      </Section>

      <Section
        title="Projects"
        action={<AddButton onClick={() => update((p) => p.projects.push({ id: "", name: "", description: null, bullets: [] }))}>Add project</AddButton>}
      >
        {draft.projects.length === 0 && <p className="text-sm text-zinc-500">None found.</p>}
        {draft.projects.map((proj, i) => (
          <div key={proj.id || `new-${i}`} className="space-y-3 rounded-lg border border-zinc-200 p-4">
            <div className="flex items-start gap-2">
              <div className="grid min-w-0 flex-1 gap-3">
                <Field label="Name">
                  <Text value={proj.name} onChange={(v) => update((p) => void (p.projects[i].name = v ?? ""))} />
                </Field>
                <Field label="Description">
                  <Text value={proj.description} onChange={(v) => update((p) => void (p.projects[i].description = v))} />
                </Field>
              </div>
              <RemoveButton label="Remove project" onClick={() => update((p) => void p.projects.splice(i, 1))} />
            </div>
            <Bullets bullets={proj.bullets} flagged={flagged} onChange={(b) => update((p) => void (p.projects[i].bullets = b))} />
          </div>
        ))}
      </Section>

      <Section
        title="Education"
        action={
          <AddButton onClick={() => update((p) => p.education.push({ id: "", school: "", degree: null, field: null, start: null, end: null }))}>
            Add school
          </AddButton>
        }
      >
        {draft.education.map((ed, i) => (
          <div key={ed.id || `new-${i}`} className="flex items-start gap-2 rounded-lg border border-zinc-200 p-4">
            <div className="grid min-w-0 flex-1 gap-3 sm:grid-cols-2">
              <Field label="School" className="sm:col-span-2">
                <Text value={ed.school} onChange={(v) => update((p) => void (p.education[i].school = v ?? ""))} />
              </Field>
              <Field label="Degree">
                <Text value={ed.degree} onChange={(v) => update((p) => void (p.education[i].degree = v))} />
              </Field>
              <Field label="Field">
                <Text value={ed.field} onChange={(v) => update((p) => void (p.education[i].field = v))} />
              </Field>
              <Field label="Start">
                <Text value={ed.start} onChange={(v) => update((p) => void (p.education[i].start = v))} />
              </Field>
              <Field label="End">
                <Text value={ed.end} onChange={(v) => update((p) => void (p.education[i].end = v))} />
              </Field>
            </div>
            <RemoveButton label="Remove school" onClick={() => update((p) => void p.education.splice(i, 1))} />
          </div>
        ))}
      </Section>

      <Section title="Skills" action={<AddButton onClick={() => update((p) => p.skills.push({ name: "", years: null, used_at: [] }))}>Add skill</AddButton>}>
        <div className="space-y-2">
          {draft.skills.map((skill, i) => (
            <div key={i} className="flex items-start gap-2">
              <div className="grid min-w-0 flex-1 grid-cols-[1fr_5.5rem] gap-2 sm:grid-cols-[1fr_6rem_1.5fr]">
                <input
                  aria-label="Skill"
                  className={input}
                  value={skill.name}
                  onChange={(e) => update((p) => void (p.skills[i].name = e.target.value))}
                />
                <input
                  aria-label="Years"
                  type="number"
                  min={0}
                  step={0.5}
                  placeholder="Years"
                  className={input}
                  value={skill.years ?? ""}
                  onChange={(e) => update((p) => void (p.skills[i].years = e.target.value === "" ? null : Number(e.target.value)))}
                />
                <input
                  aria-label="Used at"
                  placeholder="Used at"
                  className={`${input} col-span-2 sm:col-span-1`}
                  value={skill.used_at.join(", ")}
                  onChange={(e) =>
                    update((p) => void (p.skills[i].used_at = e.target.value.split(",").map((v) => v.trim()).filter(Boolean)))
                  }
                />
              </div>
              <RemoveButton label="Remove skill" onClick={() => update((p) => void p.skills.splice(i, 1))} />
            </div>
          ))}
        </div>
      </Section>

      <Section title="Certifications, languages and awards">
        <div className="grid gap-3">
          <Field label="Certifications">
            <ChipInput id="certifications" values={draft.certifications} onChange={(v) => update((p) => void (p.certifications = v))} />
          </Field>
          <Field label="Languages">
            <ChipInput id="languages" values={draft.languages} onChange={(v) => update((p) => void (p.languages = v))} />
          </Field>
          <Field label="Awards">
            <ChipInput id="awards" values={draft.awards} onChange={(v) => update((p) => void (p.awards = v))} />
          </Field>
        </div>
      </Section>

      <div className="sticky bottom-16 flex flex-wrap items-center justify-end gap-3 border-t border-zinc-200 bg-white/95 px-5 py-3 backdrop-blur md:bottom-0">
        {record.status === "confirmed" && !dirty ? (
          <span className="mr-auto inline-flex items-center gap-1.5 text-sm font-medium text-emerald-700">
            <CheckCircle2 className="size-4" />
            Confirmed
          </span>
        ) : (
          <span className="mr-auto text-sm text-zinc-500">{dirty ? "Unsaved changes" : "Check everything, then confirm."}</span>
        )}
        <Button icon={Save} onClick={save} disabled={!dirty} loading={busy && dirty}>
          Save
        </Button>
        <Button
          variant="primary"
          icon={CheckCircle2}
          disabled={record.status === "confirmed" && !dirty}
          loading={busy && !dirty}
          onClick={() =>
            run(async () => {
              if (dirty) {
                await onSave(draft);
                setDirty(false);
              }
              await onConfirm();
            })
          }
        >
          {dirty ? "Save and confirm" : "Confirm"}
        </Button>
      </div>
    </div>
  );
}
