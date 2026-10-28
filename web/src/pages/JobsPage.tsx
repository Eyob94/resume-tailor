import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  ArrowUpDown,
  Bookmark,
  BookmarkCheck,
  BookmarkPlus,
  Braces,
  CheckCircle2,
  ChevronDown,
  Circle,
  CircleDot,
  CircleSlash,
  Check,
  Copy,
  Building2,
  ExternalLink,
  EyeOff,
  Globe,
  Inbox,
  Layers,
  Loader2,
  MoreHorizontal,
  Play,
  Plus,
  RotateCcw,
  RotateCw,
  Search,
  Send,
  Trash2,
  XCircle,
} from "lucide-react";
import {
  errorMessage,
  jobsApi,
  profileApi,
  type JobListing,
  type JobScore,
  type JobSearch,
  type JobStatus,
  type ScanStatus,
  type SearchInput,
  type Site,
} from "../api";
import { Dialog } from "../components/Dialog";
import { JsonView } from "../components/JsonView";
import { SearchForm, WORK_MODE_LABELS } from "../components/SearchForm";
import { Alert, Button, Card, EmptyState, Menu, PageHeader, Skeleton } from "../components/ui";
import { useDevMode } from "../devMode";
import { plural, postedLabel, relativeTime } from "../format";

type Filter = JobStatus | "all";
const FILTERS: [Filter, string, typeof Inbox][] = [
  ["new", "New", Inbox],
  ["saved", "Saved", Bookmark],
  ["applied", "Applied", Send],
  ["dismissed", "Dismissed", EyeOff],
  ["all", "All", Layers],
];
const SITE_LABELS: Record<Site, string> = { linkedin: "LinkedIn", indeed: "Indeed" };

export function JobsPage() {
  const [searches, setSearches] = useState<JobSearch[] | null>(null);
  const [jobs, setJobs] = useState<JobListing[] | null>(null);
  const [scans, setScans] = useState<ScanStatus[] | null>(null);
  const [editing, setEditing] = useState<JobSearch | "new" | null>(null);
  const [pendingEdit, setPendingEdit] = useState<{ search: JobSearch; input: SearchInput; newJobIds: number[] } | null>(null);
  const [applyingEdit, setApplyingEdit] = useState<"save" | "discard" | null>(null);
  const [filter, setFilter] = useState<Filter>("new");
  const [sort, setSort] = useState<"newest" | "match">("newest");
  const [openJobId, setOpenJobId] = useState<number | null>(null);
  const [filtersOpen, setFiltersOpen] = useState(() => {
    try {
      return localStorage.getItem("jobs.filtersOpen") === "true";
    } catch {
      return false;
    }
  });
  const [resumeConfirmed, setResumeConfirmed] = useState(false);
  const [devMode] = useDevMode();
  const [copiedUrl, setCopiedUrl] = useState(false);

  useEffect(() => {
    try {
      localStorage.setItem("jobs.filtersOpen", String(filtersOpen));
    } catch {
    }
  }, [filtersOpen]);

  useEffect(() => {
    profileApi.get().then(
      (record) => setResumeConfirmed(record?.status === "confirmed"),
      () => setResumeConfirmed(false),
    );
  }, []);
  const [error, setError] = useState<string | null>(null);

  const loadJobs = useCallback(async () => setJobs(await jobsApi.listJobs()), []);

  useEffect(() => {
    Promise.all([
      jobsApi.listSearches().then((all) => {
        setSearches(all);
      }),
      loadJobs(),
      jobsApi.scanStatus().then(setScans),
    ]).catch((e) =>
      setError(errorMessage(e)),
    );
  }, [loadJobs]);

  const lastFinished = useRef<string | undefined>(undefined);
  useEffect(() => {
    const id = setInterval(async () => {
      try {
        const statuses = await jobsApi.scanStatus();
        setScans(statuses);
        const finished = statuses.map((s) => s.last_run?.finished_at ?? "").join("|");
        if (lastFinished.current !== undefined && finished !== lastFinished.current) await loadJobs();
        lastFinished.current = finished;
      } catch {
      }
    }, 5_000);
    return () => clearInterval(id);
  }, [loadJobs]);

  const scoringPending =
    resumeConfirmed && (jobs ?? []).some((j) => j.status === "saved" && j.description && (!j.score || j.score.status === "scoring"));
  useEffect(() => {
    if (!scoringPending) return;
    const id = setInterval(() => void loadJobs().catch(() => undefined), 5_000);
    return () => clearInterval(id);
  }, [scoringPending, loadJobs]);

  async function skipScore(job: JobListing) {
    try {
      await jobsApi.skipScore(job.id);
      await loadJobs();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function rescore(job: JobListing) {
    try {
      await jobsApi.rescore(job.id);
      await loadJobs();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function saveSearch(input: SearchInput) {
    if (editing && editing !== "new") {
      const newJobIds = (jobs ?? []).filter((j) => j.status === "new" && j.search_ids.includes(editing.id)).map((j) => j.id);
      if (newJobIds.length > 0) {
        setPendingEdit({ search: editing, input, newJobIds });
        return;
      }
      await applyEdit(editing, input);
      return;
    }
    const created = await jobsApi.createSearch(input);
    setSearches((all) => [...(all ?? []), created]);
    setEditing(null);
    setScans(await jobsApi.scanStatus());
  }

  async function applyEdit(search: JobSearch, input: SearchInput) {
    const updated = await jobsApi.updateSearch(search.id, input);
    setSearches((all) => all?.map((s) => (s.id === updated.id ? updated : s)) ?? null);
    await loadJobs();
    setEditing(null);
    setScans(await jobsApi.scanStatus());
  }

  async function finishPendingEdit(saveJobs: boolean) {
    if (!pendingEdit) return;
    setApplyingEdit(saveJobs ? "save" : "discard");
    try {
      if (saveJobs) await jobsApi.setJobsStatus(pendingEdit.newJobIds, "saved");
      await applyEdit(pendingEdit.search, pendingEdit.input);
      setPendingEdit(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setApplyingEdit(null);
    }
  }

  async function removeSearch(search: JobSearch) {
    if (!confirm(`Delete the preference "${search.name}"? Jobs matched only by it will disappear from the list.`)) return false;
    try {
      await jobsApi.deleteSearch(search.id);
      setSearches((all) => all?.filter((s) => s.id !== search.id) ?? null);
      await loadJobs();
      return true;
    } catch (e) {
      setError(errorMessage(e));
      return false;
    }
  }

  async function copyLinkedinUrl(search: JobSearch) {
    try {
      await navigator.clipboard.writeText(linkedinSearchUrl(search));
      setCopiedUrl(true);
      window.setTimeout(() => setCopiedUrl(false), 2000);
    } catch {
      setCopiedUrl(false);
    }
  }

  async function runScan(site: Site) {
    setScans((all) => all?.map((scan) => scan.site === site ? { ...scan, running: true } : scan) ?? null);
    try {
      await jobsApi.runScan(site);
      window.setTimeout(() => {
        void jobsApi.scanStatus().then(setScans).catch((e) => setError(errorMessage(e)));
      }, 700);
    } catch (e) {
      setError(errorMessage(e));
      void jobsApi.scanStatus().then(setScans).catch(() => undefined);
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

  async function toggleSearch(search: JobSearch) {
    const enabled = !search.enabled;
    setSearches((all) => all?.map((item) => (item.id === search.id ? { ...item, enabled } : item)) ?? null);
    try {
      const updated = await jobsApi.setSearchEnabled(search.id, enabled);
      setSearches((all) => all?.map((item) => (item.id === updated.id ? updated : item)) ?? null);
    } catch (e) {
      setSearches((all) => all?.map((item) => (item.id === search.id ? { ...item, enabled: search.enabled } : item)) ?? null);
      setError(errorMessage(e));
    }
  }

  const enabledSearchIds = new Set(searches?.filter((search) => search.enabled).map((search) => search.id) ?? []);
  const matchingJobs = jobs?.filter((job) => job.search_ids.some((id) => enabledSearchIds.has(id))) ?? [];
  const counts = Object.fromEntries(FILTERS.map(([f]) => [f, matchingJobs.filter((j) => f === "all" || j.status === f).length]));
  const filtered = matchingJobs.filter((j) => filter === "all" || j.status === filter);
  const matchScore = (j: JobListing) => (j.score?.status === "ready" ? (j.score.score ?? -1) : -1);
  const visible = sort === "match" ? [...filtered].sort((a, b) => matchScore(b) - matchScore(a)) : filtered;

  useEffect(() => {
    if (openJobId === null) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpenJobId(null);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [openJobId]);

  return (
    <>
      <PageHeader
        title="Jobs"
        actions={
          searches?.length && !editing ? (
            <Button variant="primary" icon={Plus} aria-label="New preference" onClick={() => setEditing("new")} className="size-9 !rounded-full p-0 sm:size-auto sm:!rounded-lg sm:px-3.5 sm:py-2">
              <span className="hidden sm:inline">New preference</span>
            </Button>
          ) : null
        }
      />
      {error && <Alert onDismiss={() => setError(null)}>{error}</Alert>}

      {searches === null ? (
        <Skeleton className="h-40" />
      ) : (
        <section className="mb-4 space-y-4">
          {editing && (
            <Card>
              <div className="flex items-center justify-between gap-3 border-b border-zinc-100 px-5 py-4">
                <h2 className="font-semibold">{editing === "new" ? "New preference" : `Edit “${editing.name}”`}</h2>
                {editing !== "new" && <div className="flex shrink-0 items-center gap-2">
                  {devMode && <>
                    <a href={linkedinSearchUrl(editing)} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-2 text-sm font-medium text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900">
                      <ExternalLink className="size-4" /> LinkedIn
                    </a>
                    <button type="button" onClick={() => copyLinkedinUrl(editing)} className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-2 text-sm font-medium text-zinc-600 hover:bg-zinc-100 hover:text-zinc-900">
                      {copiedUrl ? <Check className="size-4" /> : <Copy className="size-4" />}{copiedUrl ? "Copied" : "Copy URL"}
                    </button>
                  </>}
                  <Button variant="danger" icon={Trash2} onClick={async () => { if (await removeSearch(editing)) setEditing(null); }}>Delete</Button>
                </div>}
              </div>
              <SearchForm initial={editing === "new" ? undefined : editing} onSubmit={saveSearch} onCancel={() => setEditing(null)} />
            </Card>
          )}

          {searches.length === 0 && !editing ? (
            <EmptyState
              icon={Search}
              title="No preferences yet"
              action={
                <Button variant="primary" icon={Plus} onClick={() => setEditing("new")}>
                  Create a preference
                </Button>
              }
            >
              Add the titles, keywords, location and work mode you want LinkedIn and Indeed checked on a schedule.
            </EmptyState>
          ) : (
            <>
              <button
                type="button"
                aria-expanded={filtersOpen}
                onClick={() => setFiltersOpen((open) => !open)}
                className="flex w-full items-center gap-2 text-left text-sm text-zinc-500 sm:hidden"
              >
                {scans?.some((scan) => scan.last_run && (scan.last_run.status === "partial" || scan.last_run.status === "failed")) && <AlertTriangle className="size-4 shrink-0 text-amber-500" />}
                {scans?.some((scan) => scan.running) && <Loader2 className="size-3.5 shrink-0 animate-spin" />}
                <span className="min-w-0 flex-1 truncate">
                  {searches.filter((search) => search.enabled).length === searches.length
                    ? plural(searches.length, "preference")
                    : `${searches.filter((search) => search.enabled).length} of ${searches.length} preferences`}
                  {" · "}
                  {scans?.some((scan) => scan.running)
                    ? "scanning…"
                    : (() => {
                        const latest = scans?.map((scan) => scan.last_run?.started_at).filter((value): value is string => Boolean(value)).sort((first, second) => Date.parse(first) - Date.parse(second)).at(-1);
                        return latest ? `scanned ${relativeTime(latest)}` : "not scanned yet";
                      })()}
                </span>
                <ChevronDown className={`size-4 shrink-0 transition-transform ${filtersOpen ? "rotate-180" : ""}`} />
              </button>
              <div className={`${filtersOpen ? "block" : "hidden"} sm:block`}>
                <div className="flex flex-wrap gap-2">
                  {searches.map((search) => (
                    <div key={search.id} className={`inline-flex items-center rounded-full border text-sm ${search.enabled ? "border-zinc-300 bg-white text-zinc-900" : "border-zinc-200 bg-zinc-50 text-zinc-400"}`}>
                      <button type="button" aria-label={`${search.enabled ? "Turn off" : "Turn on"} scans for ${search.name}`} onClick={() => void toggleSearch(search)} className="py-1 pl-2.5 pr-1">
                        {search.enabled ? <CheckCircle2 className="size-4 text-emerald-600" /> : <Circle className="size-4 text-zinc-300" />}
                      </button>
                      <button
                        type="button"
                        onClick={() => setEditing(search)}
                        title={`${[...search.titles, ...search.keywords].join(", ")} · ${search.location ?? "Anywhere"} · past ${search.posted_within}`}
                        className="max-w-56 truncate py-1 pl-1 pr-3 text-left"
                      >
                        {search.name}
                      </button>
                    </div>
                  ))}
                </div>
                {scans && searches.length > 0 && <ScanLine scans={scans} onRun={runScan} />}
              </div>
            </>
          )}
        </section>
      )}

      {searches && searches.length > 0 && (
        <section>
          <div className="flex items-center justify-between gap-2 py-3 sm:gap-3 sm:border-b sm:border-zinc-200 sm:pb-3 sm:pt-0">
            <div className="hide-scrollbar inline-flex min-w-0 max-w-full items-center gap-1 overflow-x-auto rounded-full bg-zinc-100 p-1" role="tablist">
              {FILTERS.map(([id, label, Icon]) => (
                <button
                  key={id}
                  role="tab"
                  aria-selected={filter === id}
                  aria-label={`${label}, ${counts[id]} jobs`}
                  onClick={() => setFilter(id)}
                  className={`inline-flex h-9 shrink-0 items-center rounded-full px-2.5 text-sm font-medium transition-all duration-300 ease-out motion-reduce:transition-none sm:px-3.5 ${
                    filter === id ? "bg-white px-3.5 text-zinc-900 shadow-sm" : "text-zinc-500 hover:text-zinc-900"
                  }`}
                >
                  <Icon className="size-4 shrink-0" />
                  <span className={`overflow-hidden whitespace-nowrap transition-all duration-300 ease-out motion-reduce:transition-none ${filter === id ? "ml-1.5 max-w-28 opacity-100" : "ml-0 max-w-0 opacity-0"} sm:ml-1.5 sm:max-w-28 sm:opacity-100`}>{label}</span>
                  <span className={`ml-1 overflow-hidden text-xs font-normal text-zinc-400 transition-all duration-300 ease-out motion-reduce:transition-none ${filter === id ? "max-w-12 opacity-100" : "max-w-0 opacity-0"} sm:max-w-none sm:opacity-100`}>{counts[id]}</span>
                </button>
              ))}
            </div>
            <div className="flex shrink-0 items-center gap-1 sm:hidden">
              <Menu
                label="Sort jobs"
                heading="Sort by"
                trigger={<span className="rounded-lg p-2 text-zinc-500 hover:bg-zinc-100"><ArrowUpDown className="size-4" /></span>}
                items={[
                  { label: "Newest", icon: sort === "newest" ? Check : null, onSelect: () => setSort("newest") },
                  { label: "Best match", icon: sort === "match" ? Check : null, onSelect: () => setSort("match") },
                ]}
              />
              {filter === "new" && visible.length > 0 && (
                <button type="button" aria-label={`Save all ${visible.length}`} title={`Save all ${visible.length}`} onClick={() => saveAll(visible.map((job) => job.id))} className="rounded-lg p-2 text-zinc-500 hover:bg-zinc-100">
                  <BookmarkPlus className="size-4" />
                </button>
              )}
            </div>
            <div className="hidden shrink-0 items-center gap-2 sm:flex">
              <select
                aria-label="Sort jobs"
                value={sort}
                onChange={(e) => setSort(e.target.value as "newest" | "match")}
                className="border-0 bg-transparent px-2 py-1.5 text-sm text-zinc-600 shadow-none"
              >
                <option value="newest">Newest</option>
                <option value="match">Best match</option>
              </select>
              {filter === "new" && visible.length > 0 && (
                <button type="button" title={`Save all ${visible.length} jobs`} onClick={() => saveAll(visible.map((j) => j.id))} className="inline-flex shrink-0 items-center gap-1 text-sm font-medium text-zinc-700 hover:text-zinc-900">
                  <BookmarkPlus className="size-4" /> Save all
                </button>
              )}
            </div>
          </div>

          {jobs === null ? (
            <Skeleton className="h-64" />
          ) : visible.length === 0 ? (
            <p className="py-12 text-center text-sm text-zinc-500">
              {filter === "new" ? "No new jobs. New matches show up here after each scan." : "Nothing here."}
            </p>
          ) : (
            <div className="divide-y divide-zinc-200 border-b border-zinc-200">
              {visible.map((job) => (
                <JobRow
                  key={job.id}
                  job={job}
                  resumeConfirmed={resumeConfirmed}
                  open={openJobId === job.id}
                  onToggle={() => setOpenJobId((current) => current === job.id ? null : job.id)}
                  onStatus={(s) => { void setStatus(job, s); setOpenJobId(null); }}
                  onRescore={() => rescore(job)}
                  onSkip={() => skipScore(job)}
                />
              ))}
            </div>
          )}
        </section>
      )}
      {openJobId !== null && <div className="fixed inset-0 z-30 bg-zinc-50/50 backdrop-blur-[2px] transition-opacity" onClick={() => setOpenJobId(null)} />}
      {pendingEdit && (
        <Dialog
          title="Start a new search?"
          onClose={() => !applyingEdit && setPendingEdit(null)}
          actions={
            <>
              <Button onClick={() => setPendingEdit(null)} disabled={applyingEdit !== null}>
                Cancel
              </Button>
              <Button variant="danger" onClick={() => finishPendingEdit(false)} loading={applyingEdit === "discard"} disabled={applyingEdit !== null}>
                Continue without saving
              </Button>
              <Button variant="primary" onClick={() => finishPendingEdit(true)} loading={applyingEdit === "save"} disabled={applyingEdit !== null}>
                Save {plural(pendingEdit.newJobIds.length, "job")} and continue
              </Button>
            </>
          }
        >
          <p>
            “{pendingEdit.search.name}” has {plural(pendingEdit.newJobIds.length, "new job")}. Changing it starts a fresh search, and new jobs you haven't
            saved will be removed. Saved and dismissed jobs are kept.
          </p>
        </Dialog>
      )}
    </>
  );
}

function ScanLine({ scans, onRun }: { scans: ScanStatus[]; onRun: (site: Site) => void }) {
  return (
    <div className="mt-3 divide-y divide-zinc-100 rounded-xl bg-white ring-1 ring-zinc-200">
      {scans.map((scan) => {
        const last = scan.last_run;
        const problem = last && (last.status === "partial" || last.status === "failed");
        return (
          <div key={scan.site} className="flex items-center gap-3 px-3 py-2.5 text-sm">
            <span className={`size-2 shrink-0 rounded-full ${scan.site === "linkedin" ? "bg-sky-500" : "bg-violet-500"}`} />
            <span className="w-16 shrink-0 font-medium text-zinc-800">{SITE_LABELS[scan.site]}</span>
            <div className="min-w-0 flex-1">
              <div className="flex min-w-0 items-center gap-1 text-zinc-500">
                {problem && <AlertTriangle className="size-3.5 shrink-0 text-amber-500" />}
                <span className="truncate">{scan.running ? "Scanning…" : last ? relativeTime(last.started_at) : "Not scanned yet"}</span>
              </div>
              {problem && last.message && <p className="truncate text-xs text-amber-700">{last.message}</p>}
            </div>
            <button
              type="button"
              aria-label={`Run ${SITE_LABELS[scan.site]} scan`}
              onClick={() => onRun(scan.site)}
              disabled={scan.running}
              className="inline-flex shrink-0 items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium text-zinc-700 ring-1 ring-zinc-200 hover:bg-zinc-50 disabled:pointer-events-none disabled:opacity-50"
            >
              {scan.running ? <Loader2 className="size-3 animate-spin" /> : <Play className="size-3" />}
              Run
            </button>
          </div>
        );
      })}
    </div>
  );
}

function linkedinSearchUrl(search: JobSearch) {
  const params = new URLSearchParams({
    keywords: search.titles[0] ?? search.keywords.join(" "),
    f_TPR: { day: "r86400", week: "r604800", month: "r2592000" }[search.posted_within],
    sortBy: "DD",
  });
  if (search.location) params.set("location", search.location);
  if (search.work_modes.length) {
    params.set("f_WT", search.work_modes.map((mode) => ({ onsite: "1", remote: "2", hybrid: "3" })[mode]).join(","));
  }
  return `https://www.linkedin.com/jobs/search?${params.toString()}`;
}

function ScoreBadge({ job, resumeConfirmed }: { job: JobListing; resumeConfirmed: boolean }) {
  const base = "inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs font-medium";
  const score = job.score;
  if (score?.status === "ready" && score.score !== null) {
    const tone =
      score.score >= 75 ? "bg-emerald-50 text-emerald-700" : score.score >= 50 ? "bg-amber-50 text-amber-800" : "bg-red-50 text-red-700";
    return <span className={`${base} ${tone}`}>{score.score}% match</span>;
  }
  if (score?.status === "scoring") {
    return (
      <span className={`${base} bg-zinc-100 text-zinc-600`}>
        <Loader2 className="size-3 animate-spin" />
        Scoring…
      </span>
    );
  }
  if (score?.status === "failed") return <span className={`${base} bg-red-50 text-red-700`}>Score failed</span>;
  if (score?.status === "skipped") return <span className={`${base} bg-zinc-100 text-zinc-500`}>Not scoring</span>;
  if (job.status !== "saved") return null;
  const hint = !resumeConfirmed ? "Confirm your resume to score" : !job.description ? "Waiting for description" : "Queued for scoring";
  return <span className={`${base} bg-zinc-100 text-zinc-500`}>{hint}</span>;
}

const VERDICT_ICONS = {
  met: <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-emerald-600" aria-label="Met" />,
  partial: <CircleDot className="mt-0.5 size-4 shrink-0 text-amber-600" aria-label="Partly met" />,
  missing: <XCircle className="mt-0.5 size-4 shrink-0 text-red-500" aria-label="Missing" />,
};

function MatchDetails({ score }: { score: JobScore }) {
  return (
    <div className="mb-4 rounded-lg border border-zinc-200 bg-zinc-50 p-3">
      <p className="text-sm font-medium">How you match</p>
      {score.summary && <p className="mt-1 text-sm text-zinc-600">{score.summary}</p>}
      <ul className="mt-3 space-y-2">
        {score.requirements.map((r) => (
          <li key={r.text} className="flex items-start gap-2 text-sm">
            {VERDICT_ICONS[r.verdict]}
            <div className="min-w-0">
              <span className="text-zinc-800">{r.text}</span>
              {r.importance === "nice" && <span className="ml-1.5 text-xs text-zinc-400">nice to have</span>}
              {r.evidence && <p className="mt-0.5 text-xs text-zinc-500">“{r.evidence}”</p>}
            </div>
          </li>
        ))}
      </ul>
      <p className="mt-3 text-xs text-zinc-400">Scored by {score.model}</p>
    </div>
  );
}

function JobRow({
  job,
  resumeConfirmed,
  open,
  onToggle,
  onStatus,
  onRescore,
  onSkip,
}: {
  job: JobListing;
  resumeConfirmed: boolean;
  open: boolean;
  onToggle: () => void;
  onStatus: (s: JobStatus) => void;
  onRescore: () => void;
  onSkip: () => void;
}) {
  const rowRef = useRef<HTMLDivElement>(null);
  const [fullDescription, setFullDescription] = useState(false);
  const [showJson, setShowJson] = useState(false);
  const [copied, setCopied] = useState(false);
  const [devMode] = useDevMode();

  useEffect(() => {
    if (open) rowRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [open]);
  const statusButtonBase = "inline-flex items-center justify-center rounded-lg p-2 transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-900 disabled:pointer-events-none disabled:opacity-50 sm:gap-2 sm:border sm:border-zinc-200 sm:bg-white sm:px-3.5 sm:py-2 sm:shadow-sm";
  const statusButtonMuted = `${statusButtonBase} text-zinc-500 hover:bg-zinc-100 sm:text-zinc-700 sm:hover:bg-zinc-50 sm:hover:text-zinc-900`;
  const statusButtonActive = `${statusButtonBase} text-zinc-900 hover:bg-zinc-100 sm:text-zinc-700 sm:hover:bg-zinc-50 sm:hover:text-zinc-900`;

  async function copyDescription() {
    if (!job.description) return;
    try {
      await navigator.clipboard.writeText(job.description);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  }

  const sourceBadge = <span className={`rounded px-1.5 py-px text-[11px] font-medium ${job.source === "linkedin" ? "bg-sky-50 text-sky-700" : "bg-violet-50 text-violet-700"}`}>{SITE_LABELS[job.source]}</span>;
  const keywordLabel = job.matched_keywords.length > 0
    ? `${job.matched_keywords.slice(0, 3).join(", ")}${job.matched_keywords.length > 3 ? ` +${job.matched_keywords.length - 3}` : ""}`
    : undefined;
  const desktopMetaParts = [
    { key: "company", value: job.company },
    { key: "city", value: job.location?.split(",")[0] },
    { key: "work-mode", value: job.work_mode ? WORK_MODE_LABELS[job.work_mode] : undefined },
    { key: "posted", value: job.posted_on ? postedLabel(job.posted_on) : undefined },
    { key: "source", value: SITE_LABELS[job.source] },
    { key: "keywords", value: keywordLabel },
  ].filter((part): part is { key: string; value: string } => Boolean(part.value));
  const expandedMetaParts = desktopMetaParts.filter((part) => part.key === "city" || part.key === "work-mode" || part.key === "source" || part.key === "keywords");

  return (
    <div
      ref={rowRef}
      className={`py-3 transition-all duration-200 sm:py-3.5 ${job.status === "dismissed" ? "opacity-60" : ""} ${open ? "relative z-40 -mx-3 rounded-2xl bg-white px-3 shadow-xl ring-1 ring-zinc-200" : ""}`}
    >
      <div className="flex items-start gap-3">
        <div
          role="button"
          tabIndex={0}
          aria-expanded={open}
          onClick={onToggle}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              onToggle();
            }
          }}
          className="min-w-0 flex-1 cursor-pointer"
        >
          <div className="flex flex-wrap items-center gap-2">
            <span className="line-clamp-2 min-w-0 flex-1 font-medium text-zinc-900 sm:line-clamp-none">{job.title}</span>
            <span className="hidden sm:inline-flex"><ScoreBadge job={job} resumeConfirmed={resumeConfirmed} /></span>
            {job.tailored_at && <span className="hidden items-center gap-1 rounded-md bg-emerald-50 px-1.5 py-0.5 text-xs font-medium text-emerald-700 sm:inline-flex"><CheckCircle2 className="size-3" />Tailored</span>}
          </div>
          <div className="mt-0.5 flex min-w-0 items-center gap-x-1.5 overflow-hidden whitespace-nowrap text-sm text-zinc-500 sm:hidden">
            <span title={SITE_LABELS[job.source]} className={`size-1.5 shrink-0 rounded-full ${job.source === "linkedin" ? "bg-sky-500" : "bg-violet-500"}`} />
            <span className="min-w-0 truncate">{job.company}</span>
            {job.posted_on && <><span className="shrink-0 text-zinc-300">·</span><span className="shrink-0">{postedLabel(job.posted_on)}</span></>}
          </div>
          <div className="mt-0.5 hidden flex-wrap items-center gap-x-1.5 text-sm text-zinc-500 sm:flex">
            {desktopMetaParts.map((part, index) => (
              <span key={part.key} className="inline-flex shrink-0 items-center gap-x-1.5 whitespace-nowrap">
                {part.key === "source" ? sourceBadge : <span className={part.key === "keywords" ? "text-emerald-700" : ""}>{part.value}</span>}
                {index < desktopMetaParts.length - 1 && <span className="text-zinc-300">·</span>}
              </span>
            ))}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-0 sm:gap-1">
          {job.status === "new" && <button type="button" aria-label={`Save ${job.title}`} onClick={() => onStatus("saved")} className={statusButtonMuted}><Bookmark className="size-4 shrink-0" /><span className="hidden sm:inline">Save</span></button>}
          {job.status === "saved" && <button type="button" aria-label={`Mark ${job.title} as new`} onClick={() => onStatus("new")} className={statusButtonActive}><BookmarkCheck className="size-4 shrink-0" /><span className="hidden sm:inline">Saved</span></button>}
          {job.status === "applied" && <button type="button" aria-label={`Undo applied for ${job.title}`} onClick={() => onStatus("new")} className={statusButtonActive}><Check className="size-4 shrink-0" /><span className="hidden sm:inline">Applied</span></button>}
          {job.status === "dismissed" && <button type="button" aria-label={`Restore ${job.title}`} onClick={() => onStatus("new")} className={statusButtonMuted}><RotateCcw className="size-4 shrink-0" /><span className="hidden sm:inline">Restore</span></button>}
          <Menu
            label={`More actions for ${job.title}`}
            trigger={<span className="rounded-lg p-2 text-zinc-500 hover:bg-zinc-100"><MoreHorizontal className="size-5" /></span>}
            items={[
              ...(job.status !== "applied" ? [{ label: "Mark applied", icon: Check, onSelect: () => onStatus("applied") }] : [{ label: "Undo applied", icon: RotateCcw, onSelect: () => onStatus("new") }]),
              ...(job.status !== "dismissed" ? [{ label: "Dismiss", icon: EyeOff, onSelect: () => onStatus("dismissed"), danger: true }] : []),
            ]}
          />
        </div>
      </div>

      {open && (
        <div className="mt-3 pb-3">
          <div className="mb-3 flex flex-wrap items-center text-xs text-zinc-500">
            {expandedMetaParts.map((part) => (
              <span key={part.key} className="after:mx-1.5 after:text-zinc-300 after:content-['·'] last:after:content-none">
                {part.value}
              </span>
            ))}
          </div>
          <div className="mb-3 flex items-center gap-2 sm:hidden">
            <ScoreBadge job={job} resumeConfirmed={resumeConfirmed} />
            {job.tailored_at && <span className="inline-flex items-center gap-1 rounded-md bg-emerald-50 px-1.5 py-0.5 text-xs font-medium text-emerald-700"><CheckCircle2 className="size-3" />Tailored</span>}
          </div>
          {job.score?.status === "ready" && <MatchDetails score={job.score} />}
          {job.score?.status === "failed" && job.score.error && <p className="mb-3 text-sm text-red-700">Scoring failed: {job.score.error}</p>}
          {job.status === "saved" && resumeConfirmed && job.description && (
            <div className="mb-4 flex flex-wrap gap-2">
              {(job.score?.status === "ready" || job.score?.status === "failed") && (
                <Button icon={RotateCw} onClick={onRescore}>
                  Score again
                </Button>
              )}
              {job.score?.status === "skipped" && (
                <Button icon={RotateCw} onClick={onRescore}>
                  Score this job
                </Button>
              )}
              {(!job.score || job.score.status === "scoring") && (
                <Button icon={CircleSlash} onClick={onSkip}>
                  {job.score?.status === "scoring" ? "Stop scoring" : "Don't score"}
                </Button>
              )}
            </div>
          )}
          {(job.seniority || job.employment_type || job.industries) && (
            <p className="mb-3 text-xs text-zinc-500">{[job.seniority, job.employment_type, job.industries].filter(Boolean).join(" · ")}</p>
          )}
          {job.description ? <>
            <p className={`whitespace-pre-wrap text-sm leading-relaxed text-zinc-700 ${fullDescription ? "" : "line-clamp-4"}`}>{job.description}</p>
            {job.description.length > 400 && <button type="button" onClick={() => setFullDescription((value) => !value)} className="mt-1 text-sm font-medium text-zinc-700 hover:text-zinc-900">{fullDescription ? "Show less" : "Show full description"}</button>}
          </> : <p className="text-sm leading-relaxed text-zinc-700">Description not fetched yet.</p>}
          <div className="hide-scrollbar -mx-3 mt-4 flex gap-2 overflow-x-auto px-3">
            <button onClick={copyDescription} disabled={!job.description} className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-zinc-100 px-3 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-200 disabled:cursor-not-allowed disabled:opacity-50" aria-label={`Copy ${job.title} description`}>
              {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}{copied ? "Copied" : "Copy"}
            </button>
            <a href={job.url} target="_blank" rel="noreferrer" className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-zinc-100 px-3 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-200"><ExternalLink className="size-3.5" />{SITE_LABELS[job.source]}</a>
            {job.company_url && <a href={job.company_url} target="_blank" rel="noreferrer" className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-zinc-100 px-3 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-200"><Building2 className="size-3.5" />Company page</a>}
            {job.company_website && <a href={job.company_website} target="_blank" rel="noreferrer" className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-zinc-100 px-3 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-200"><Globe className="size-3.5" />Company site</a>}
            {devMode && <button onClick={() => setShowJson((value) => !value)} className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-zinc-100 px-3 py-1.5 text-xs font-medium text-zinc-700 hover:bg-zinc-200"><Braces className="size-3.5" />JSON</button>}
          </div>
          {devMode && showJson && <div className="mt-3"><JsonView value={job} fileName={`job-${job.source_id}.json`} note="Stored job record" /></div>}
        </div>
      )}
    </div>
  );
}
