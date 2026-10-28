import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
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
  Loader2,
  MoreHorizontal,
  Play,
  Plus,
  RotateCcw,
  RotateCw,
  Search,
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
const FILTERS: [Filter, string][] = [
  ["new", "New"],
  ["saved", "Saved"],
  ["dismissed", "Dismissed"],
  ["applied", "Applied"],
  ["all", "All"],
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
  const [resumeConfirmed, setResumeConfirmed] = useState(false);
  const [devMode] = useDevMode();
  const [copiedUrl, setCopiedUrl] = useState(false);

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
    try {
      await jobsApi.runScan(site);
      window.setTimeout(() => {
        void jobsApi.scanStatus().then(setScans).catch((e) => setError(errorMessage(e)));
      }, 700);
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

  return (
    <>
      <PageHeader
        title="Jobs"
        actions={
          searches?.length && !editing ? (
            <Button variant="primary" icon={Plus} onClick={() => setEditing("new")}>
              New preference
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
                  <span className="text-xs font-normal text-zinc-400">{counts[id]}</span>
                </button>
              ))}
            </div>
            <select
              aria-label="Sort jobs"
              value={sort}
              onChange={(e) => setSort(e.target.value as "newest" | "match")}
              className="mb-2 shrink-0 border-0 bg-transparent px-2 py-1.5 text-sm text-zinc-600 shadow-none"
            >
              <option value="newest">Newest</option>
              <option value="match">Best match</option>
            </select>
            {filter === "new" && visible.length > 0 && (
              <button type="button" title={`Save all ${visible.length} jobs`} onClick={() => saveAll(visible.map((j) => j.id))} className="mb-2 inline-flex shrink-0 items-center gap-1 text-sm font-medium text-zinc-700 hover:text-zinc-900">
                <BookmarkPlus className="size-4" /> Save all
              </button>
            )}
          </div>

          {jobs === null ? (
            <Skeleton className="h-64" />
          ) : visible.length === 0 ? (
            <p className="py-12 text-center text-sm text-zinc-500">
              {filter === "new" ? "No new jobs. New matches show up here after each scan." : "Nothing here."}
            </p>
          ) : (
            <div className="divide-y divide-zinc-200 border-y border-zinc-200">
              {visible.map((job) => (
                <JobRow key={job.id} job={job} resumeConfirmed={resumeConfirmed} onStatus={(s) => setStatus(job, s)} onRescore={() => rescore(job)} onSkip={() => skipScore(job)} />
              ))}
            </div>
          )}
        </section>
      )}
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
  const details = (scan: ScanStatus) => {
    const last = scan.last_run;
    return [
      last?.message,
      last && `${last.new_matches} new ${last.new_matches === 1 ? "match" : "matches"}`,
      scan.next_run_at && `Next scan ${relativeTime(scan.next_run_at)}`,
      `Every ${scan.interval_minutes} min`,
    ].filter(Boolean).join(" · ");
  };

  return (
    <div className="mt-3 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-zinc-500">
      {scans.map((scan, index) => {
        const last = scan.last_run;
        const problem = last && (last.status === "partial" || last.status === "failed");
        return <span key={scan.site} className="contents">
          {index > 0 && <span className="text-zinc-300">·</span>}
          <span title={details(scan)} className="inline-flex items-center gap-1">
            {scan.running ? <><Loader2 className="size-3.5 animate-spin" />{SITE_LABELS[scan.site]} scanning…</> : problem ? <><AlertTriangle className="size-3.5 text-amber-500" />{SITE_LABELS[scan.site]} {relativeTime(last.started_at)}</> : <>{SITE_LABELS[scan.site]} {last ? relativeTime(last.started_at) : "not scanned yet"}</>}
          </span>
        </span>;
      })}
      {scans.length > 0 && <span className="text-zinc-300">·</span>}
      <Menu
        label="Run a scan now"
        align="left"
        trigger={<span className="inline-flex items-center gap-1 font-medium text-zinc-700 hover:text-zinc-900"><Play className="size-3.5" />Run now<ChevronDown className="size-3.5" /></span>}
        items={scans.map((scan) => ({ label: SITE_LABELS[scan.site], onSelect: () => onRun(scan.site), disabled: scan.running }))}
      />
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
  onStatus,
  onRescore,
  onSkip,
}: {
  job: JobListing;
  resumeConfirmed: boolean;
  onStatus: (s: JobStatus) => void;
  onRescore: () => void;
  onSkip: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [fullDescription, setFullDescription] = useState(false);
  const [showJson, setShowJson] = useState(false);
  const [copied, setCopied] = useState(false);
  const [devMode] = useDevMode();

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

  return (
    <div className={`py-3.5 ${job.status === "dismissed" ? "opacity-60" : ""}`}>
      <div className="flex items-start gap-3">
        <div
          role="button"
          tabIndex={0}
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              setOpen((value) => !value);
            }
          }}
          className="min-w-0 flex-1 cursor-pointer"
        >
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-zinc-900">{job.title}</span>
            <ScoreBadge job={job} resumeConfirmed={resumeConfirmed} />
            {job.tailored_at && <span className="inline-flex items-center gap-1 rounded-md bg-emerald-50 px-1.5 py-0.5 text-xs font-medium text-emerald-700"><CheckCircle2 className="size-3" />Tailored</span>}
          </div>
          <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 text-sm text-zinc-500">
            {[
              job.company,
              job.location?.split(",")[0],
              job.work_mode ? WORK_MODE_LABELS[job.work_mode] : null,
              job.posted_on ? postedLabel(job.posted_on) : null,
            ].filter(Boolean).map((part, index) => <span key={`${part}-${index}`} className="contents">{index > 0 && <span className="text-zinc-300">·</span>}{part}</span>)}
            <span className="text-zinc-300">·</span>
            <span className={`rounded px-1.5 py-px text-[11px] font-medium ${job.source === "linkedin" ? "bg-sky-50 text-sky-700" : "bg-violet-50 text-violet-700"}`}>{SITE_LABELS[job.source]}</span>
            {job.matched_keywords.length > 0 && <>
              <span className="text-zinc-300">·</span>
              <span className="text-emerald-700">{job.matched_keywords.slice(0, 3).join(", ")}{job.matched_keywords.length > 3 ? ` +${job.matched_keywords.length - 3}` : ""}</span>
            </>}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {job.status === "new" && <Button aria-label={`Save ${job.title}`} icon={Bookmark} onClick={() => onStatus("saved")} className="px-2.5 sm:px-3.5"><span className="hidden sm:inline">Save</span></Button>}
          {job.status === "saved" && <Button aria-label={`Mark ${job.title} as new`} icon={BookmarkCheck} onClick={() => onStatus("new")} className="px-2.5 sm:px-3.5"><span className="hidden sm:inline">Saved</span></Button>}
          {job.status === "applied" && <Button aria-label={`Undo applied for ${job.title}`} icon={Check} onClick={() => onStatus("new")} className="px-2.5 sm:px-3.5"><span className="hidden sm:inline">Applied</span></Button>}
          {job.status === "dismissed" && <Button aria-label={`Restore ${job.title}`} icon={RotateCcw} onClick={() => onStatus("new")} className="px-2.5 sm:px-3.5"><span className="hidden sm:inline">Restore</span></Button>}
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
        <div className="mt-3 rounded-lg bg-zinc-50 p-4">
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
          <div className="mt-3 flex flex-wrap items-center gap-4 text-sm text-zinc-500">
            <button onClick={copyDescription} disabled={!job.description} className="inline-flex items-center gap-1 hover:text-zinc-900 disabled:cursor-not-allowed disabled:opacity-50" aria-label={`Copy ${job.title} description`}>
              {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}{copied ? "Copied" : "Copy description"}
            </button>
            <a href={job.url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 hover:text-zinc-900"><ExternalLink className="size-3.5" />{SITE_LABELS[job.source]}</a>
            {job.company_url && <a href={job.company_url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 hover:text-zinc-900"><Building2 className="size-3.5" />Company page</a>}
            {job.company_website && <a href={job.company_website} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 hover:text-zinc-900"><Globe className="size-3.5" />Company site</a>}
            {devMode && <button onClick={() => setShowJson((value) => !value)} className="inline-flex items-center gap-1 hover:text-zinc-900"><Braces className="size-3.5" />JSON</button>}
          </div>
          {devMode && showJson && <div className="mt-3"><JsonView value={job} fileName={`job-${job.source_id}.json`} note="Stored job record" /></div>}
        </div>
      )}
    </div>
  );
}
