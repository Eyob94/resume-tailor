import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  Bookmark,
  BookmarkCheck,
  BookmarkPlus,
  Braces,
  CheckCircle2,
  ChevronDown,
  Clock,
  ExternalLink,
  EyeOff,
  Loader2,
  MapPin,
  Pencil,
  Play,
  Plus,
  RotateCcw,
  Search,
  Trash2,
} from "lucide-react";
import { errorMessage, jobsApi, type JobListing, type JobSearch, type JobStatus, type ScanStatus } from "../api";
import { JsonView } from "../components/JsonView";
import { SearchForm, WORK_MODE_LABELS } from "../components/SearchForm";
import { Alert, Button, Card, EmptyState, PageHeader, Skeleton } from "../components/ui";
import { useDevMode } from "../devMode";
import { plural, postedLabel, relativeTime } from "../format";

type Filter = JobStatus | "all";
const FILTERS: [Filter, string][] = [
  ["new", "New"],
  ["saved", "Saved"],
  ["dismissed", "Dismissed"],
  ["all", "All"],
];
const POSTED_SHORT = { day: "24 h", week: "week", month: "month" } as const;

export function JobsPage() {
  const [searches, setSearches] = useState<JobSearch[] | null>(null);
  const [jobs, setJobs] = useState<JobListing[] | null>(null);
  const [scan, setScan] = useState<ScanStatus | null>(null);
  const [editing, setEditing] = useState<JobSearch | "new" | null>(null);
  const [filter, setFilter] = useState<Filter>("new");
  const [error, setError] = useState<string | null>(null);

  const loadJobs = useCallback(async () => setJobs(await jobsApi.listJobs()), []);

  useEffect(() => {
    Promise.all([jobsApi.listSearches().then(setSearches), loadJobs(), jobsApi.scanStatus().then(setScan)]).catch((e) =>
      setError(errorMessage(e)),
    );
  }, [loadJobs]);

  // Poll scan status; reload jobs whenever a scan finishes.
  const lastFinished = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    const id = setInterval(async () => {
      try {
        const status = await jobsApi.scanStatus();
        setScan(status);
        const finished = status.last_run?.finished_at ?? null;
        if (lastFinished.current !== undefined && finished !== lastFinished.current) await loadJobs();
        lastFinished.current = finished;
      } catch {
        // Server status is shown in the sidebar; keep polling.
      }
    }, 5_000);
    return () => clearInterval(id);
  }, [loadJobs]);

  async function saveSearch(input: Parameters<typeof jobsApi.createSearch>[0]) {
    if (editing && editing !== "new") {
      const updated = await jobsApi.updateSearch(editing.id, input);
      setSearches((all) => all?.map((s) => (s.id === updated.id ? updated : s)) ?? null);
      await loadJobs();
    } else {
      const created = await jobsApi.createSearch(input);
      setSearches((all) => [...(all ?? []), created]);
    }
    setEditing(null);
    setScan(await jobsApi.scanStatus());
  }

  async function removeSearch(search: JobSearch) {
    if (!confirm(`Delete the search "${search.name}"? Jobs only it matched will disappear from the list.`)) return;
    try {
      await jobsApi.deleteSearch(search.id);
      setSearches((all) => all?.filter((s) => s.id !== search.id) ?? null);
      await loadJobs();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function saveAll(ids: number[]) {
    if (!confirm(`Save all ${plural(ids.length, "job")} in this list?`)) return;
    const idSet = new Set(ids);
    setJobs((all) => all?.map((j) => (idSet.has(j.id) ? { ...j, status: "saved" } : j)) ?? null);
    try {
      await jobsApi.setJobsStatus(ids, "saved");
    } catch (e) {
      setError(errorMessage(e));
      await loadJobs();
    }
  }

  async function setStatus(job: JobListing, status: JobStatus) {
    setJobs((all) => all?.map((j) => (j.id === job.id ? { ...j, status } : j)) ?? null);
    try {
      await jobsApi.setJobStatus(job.id, status);
    } catch (e) {
      setError(errorMessage(e));
      await loadJobs();
    }
  }

  const counts = Object.fromEntries(FILTERS.map(([f]) => [f, jobs?.filter((j) => f === "all" || j.status === f).length ?? 0]));
  const visible = jobs?.filter((j) => filter === "all" || j.status === filter) ?? [];

  return (
    <>
      <PageHeader
        title="Jobs"
        description="LinkedIn jobs matching your saved searches."
        actions={
          searches?.length && !editing ? (
            <Button variant="primary" icon={Plus} onClick={() => setEditing("new")}>
              New search
            </Button>
          ) : null
        }
      />
      {error && <Alert onDismiss={() => setError(null)}>{error}</Alert>}

      {searches === null ? (
        <Skeleton className="h-40" />
      ) : (
        <section className="mb-10 space-y-4">
          {editing && (
            <Card>
              <h2 className="border-b border-zinc-100 px-5 py-4 font-semibold">{editing === "new" ? "New search" : `Edit “${editing.name}”`}</h2>
              <SearchForm initial={editing === "new" ? undefined : editing} onSubmit={saveSearch} onCancel={() => setEditing(null)} />
            </Card>
          )}

          {searches.length === 0 && !editing ? (
            <EmptyState
              icon={Search}
              title="No saved searches yet"
              action={
                <Button variant="primary" icon={Plus} onClick={() => setEditing("new")}>
                  Create a search
                </Button>
              }
            >
              Tell it the titles, keywords, location and work mode you're after. It checks LinkedIn on a schedule.
            </EmptyState>
          ) : (
            <>
              {scan && searches.length > 0 && <ScanBar scan={scan} onChange={setScan} onError={setError} />}
              <div className="grid gap-3 sm:grid-cols-2">
                {searches.map((search) => (
                  <SearchCard key={search.id} search={search} onEdit={() => setEditing(search)} onDelete={() => removeSearch(search)} />
                ))}
              </div>
            </>
          )}
        </section>
      )}

      {searches && searches.length > 0 && (
        <section>
          <div className="mb-4 flex items-end gap-3 border-b border-zinc-200">
            <div className="flex min-w-0 flex-1 gap-1 overflow-x-auto" role="tablist">
              {FILTERS.map(([id, label]) => (
                <button
                  key={id}
                  role="tab"
                  aria-selected={filter === id}
                  onClick={() => setFilter(id)}
                  className={`-mb-px flex shrink-0 items-center gap-2 border-b-2 px-3 pb-2.5 text-sm font-medium transition-colors ${
                    filter === id ? "border-zinc-900 text-zinc-900" : "border-transparent text-zinc-500 hover:text-zinc-900"
                  }`}
                >
                  {label}
                  <span className="rounded-full bg-zinc-100 px-1.5 py-0.5 text-xs text-zinc-500">{counts[id]}</span>
                </button>
              ))}
            </div>
            {filter === "new" && visible.length > 0 && (
              <Button icon={BookmarkPlus} onClick={() => saveAll(visible.map((j) => j.id))} className="mb-2 shrink-0">
                Save all <span className="hidden sm:inline">({visible.length})</span>
              </Button>
            )}
          </div>

          {jobs === null ? (
            <Skeleton className="h-64" />
          ) : visible.length === 0 ? (
            <p className="py-12 text-center text-sm text-zinc-500">
              {filter === "new" ? "No new jobs. New matches show up here after each scan." : "Nothing here."}
            </p>
          ) : (
            <div className="space-y-3">
              {visible.map((job) => (
                <JobRow key={job.id} job={job} onStatus={(s) => setStatus(job, s)} />
              ))}
            </div>
          )}
        </section>
      )}
    </>
  );
}

function ScanBar({ scan, onChange, onError }: { scan: ScanStatus; onChange: (s: ScanStatus) => void; onError: (e: string) => void }) {
  const [requesting, setRequesting] = useState(false);
  const last = scan.last_run;
  const problem = last && (last.status === "partial" || last.status === "failed");

  async function runNow() {
    setRequesting(true);
    try {
      await jobsApi.runScan();
      // The scanner picks the request up within a moment.
      setTimeout(async () => onChange(await jobsApi.scanStatus()), 700);
    } catch (e) {
      onError(errorMessage(e));
    } finally {
      setRequesting(false);
    }
  }

  return (
    <Card className="flex flex-wrap items-center gap-x-6 gap-y-3 px-5 py-4">
      <div className="flex min-w-0 flex-1 items-start gap-3">
        {scan.running ? (
          <Loader2 className="mt-0.5 size-4 shrink-0 animate-spin text-zinc-500" />
        ) : problem ? (
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-500" />
        ) : (
          <Clock className="mt-0.5 size-4 shrink-0 text-zinc-400" />
        )}
        <div className="min-w-0 text-sm">
          <p className="font-medium">
            {scan.running
              ? "Scanning LinkedIn…"
              : last
                ? `Last scan ${relativeTime(last.started_at)} · ${plural(last.new_matches, "new match", "new matches")}`
                : "No scans yet"}
          </p>
          <p className="text-zinc-500">
            {last?.message && !scan.running ? <span className={problem ? "text-amber-700" : ""}>{last.message} </span> : null}
            {!scan.running && scan.next_run_at && `Next scan ${relativeTime(scan.next_run_at)}. `}
            Every {scan.interval_minutes} min · change in Settings.
          </p>
        </div>
      </div>
      <Button icon={Play} onClick={runNow} loading={requesting} disabled={scan.running}>
        Run now
      </Button>
    </Card>
  );
}

function SearchCard({ search, onEdit, onDelete }: { search: JobSearch; onEdit: () => void; onDelete: () => void }) {
  return (
    <Card className="flex flex-col gap-3 p-4">
      <div className="flex items-start justify-between gap-3">
        <h3 className="font-medium">{search.name}</h3>
        <div className="-mt-1 -mr-1 flex shrink-0">
          <button onClick={onEdit} className="rounded-md p-1.5 text-zinc-400 hover:bg-zinc-100 hover:text-zinc-700" aria-label={`Edit ${search.name}`}>
            <Pencil className="size-4" />
          </button>
          <button onClick={onDelete} className="rounded-md p-1.5 text-zinc-400 hover:bg-red-50 hover:text-red-600" aria-label={`Delete ${search.name}`}>
            <Trash2 className="size-4" />
          </button>
        </div>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {search.titles.map((t) => (
          <span key={t} className="rounded-md bg-zinc-900 px-2 py-0.5 text-xs font-medium text-white">
            {t}
          </span>
        ))}
        {search.keywords.map((k) => (
          <span key={k} className="rounded-md bg-zinc-100 px-2 py-0.5 text-xs text-zinc-700">
            {k}
          </span>
        ))}
      </div>
      <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-zinc-500">
        <span className="inline-flex items-center gap-1">
          <MapPin className="size-3.5" />
          {search.location ?? "Anywhere"}
        </span>
        <span>{search.work_modes.map((m) => WORK_MODE_LABELS[m]).join(" · ")}</span>
        <span>Past {POSTED_SHORT[search.posted_within]}</span>
      </p>
    </Card>
  );
}

function JobRow({ job, onStatus }: { job: JobListing; onStatus: (s: JobStatus) => void }) {
  const [open, setOpen] = useState(false);
  const [showJson, setShowJson] = useState(false);
  const [devMode] = useDevMode();

  return (
    <Card className={job.status === "dismissed" ? "opacity-60" : ""}>
      <div className="flex flex-wrap items-start gap-x-4 gap-y-3 p-4">
        <div className="min-w-0 flex-1">
          <a href={job.url} target="_blank" rel="noreferrer" className="font-medium hover:underline">
            {job.title}
          </a>
          {job.tailored_at && (
            <span className="ml-2 inline-flex items-center gap-1 rounded-md bg-emerald-50 px-1.5 py-0.5 align-middle text-xs font-medium text-emerald-700">
              <CheckCircle2 className="size-3" />
              Tailored
            </span>
          )}
          <p className="mt-0.5 text-sm text-zinc-600">
            {job.company_url ? (
              <a href={job.company_url} target="_blank" rel="noreferrer" className="hover:underline">
                {job.company}
              </a>
            ) : (
              job.company
            )}
          </p>
          <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-zinc-500">
            <span className="inline-flex items-center gap-1">
              <MapPin className="size-3.5" />
              {job.location}
            </span>
            {job.posted_on && <span>Posted {postedLabel(job.posted_on)}</span>}
            <span
              className={`rounded-md px-1.5 py-0.5 font-medium ${job.work_mode ? "bg-sky-50 text-sky-700" : "bg-zinc-100 text-zinc-500"}`}
              title={job.work_mode ? "Read from the posting text" : "The posting doesn't say"}
            >
              {job.work_mode ? WORK_MODE_LABELS[job.work_mode] : "Work mode not specified"}
            </span>
          </p>
          {job.matched_keywords.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {job.matched_keywords.map((k) => (
                <span key={k} className="rounded-md bg-emerald-50 px-2 py-0.5 text-xs font-medium text-emerald-700">
                  {k}
                </span>
              ))}
            </div>
          )}
        </div>
        <div className="flex gap-2">
          {job.status === "saved" ? (
            <Button icon={BookmarkCheck} onClick={() => onStatus("new")}>
              Saved
            </Button>
          ) : (
            <Button icon={Bookmark} onClick={() => onStatus("saved")}>
              Save
            </Button>
          )}
          {job.status === "dismissed" ? (
            <Button icon={RotateCcw} onClick={() => onStatus("new")}>
              Restore
            </Button>
          ) : (
            <Button variant="danger" icon={EyeOff} onClick={() => onStatus("dismissed")} aria-label="Dismiss">
              <span className="hidden sm:inline">Dismiss</span>
            </Button>
          )}
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-4 border-t border-zinc-100 px-4 py-2 text-sm">
        <button onClick={() => setOpen(!open)} className="inline-flex items-center gap-1 font-medium text-zinc-600 hover:text-zinc-900" aria-expanded={open}>
          <ChevronDown className={`size-4 transition-transform ${open ? "rotate-180" : ""}`} />
          {open ? "Hide description" : "Description"}
        </button>
        <a href={job.url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 text-zinc-500 hover:text-zinc-900">
          <ExternalLink className="size-3.5" />
          LinkedIn
        </a>
        {devMode && (
          <button onClick={() => setShowJson(!showJson)} className="inline-flex items-center gap-1 text-zinc-500 hover:text-zinc-900">
            <Braces className="size-3.5" />
            JSON
          </button>
        )}
      </div>

      {open && (
        <div className="border-t border-zinc-100 px-4 py-4">
          {(job.seniority || job.employment_type || job.industries) && (
            <p className="mb-3 text-xs text-zinc-500">{[job.seniority, job.employment_type, job.industries].filter(Boolean).join(" · ")}</p>
          )}
          <p className="text-sm leading-relaxed whitespace-pre-wrap text-zinc-700">{job.description ?? "Description not fetched yet."}</p>
        </div>
      )}
      {devMode && showJson && (
        <div className="border-t border-zinc-100">
          <JsonView value={job} fileName={`job-${job.linkedin_id}.json`} note="Stored job record" />
        </div>
      )}
    </Card>
  );
}
