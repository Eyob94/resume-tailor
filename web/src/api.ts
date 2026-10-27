import { useCallback, useEffect, useState } from "react";

export interface MasterResume {
  file_name: string;
  size_bytes: number;
  page_count: number;
  extracted_text: string;
  uploaded_at: string;
}

export const MAX_UPLOAD_BYTES = 10 * 1024 * 1024;

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`/api${path}`, init);
  } catch {
    throw new ApiError("Can't reach the server. Is it running?", 0);
  }
  if (!res.ok) {
    let message = res.status === 413 ? "That file is too large. The limit is 10 MB." : `Request failed (${res.status}).`;
    try {
      const body = await res.json();
      if (typeof body?.error === "string") message = body.error;
    } catch {
    }
    throw new ApiError(message, res.status);
  }
  if (res.status === 202 || res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

export const api = {
  health: () => request<{ ok: boolean }>("/health"),
  currentUser: () => request<{ username: string }>("/auth/me"),
  register: (username: string, password: string) => request<{ username: string }>("/auth/register", { method: "POST", ...json({ username, password }) }),
  login: (username: string, password: string) => request<{ username: string }>("/auth/login", { method: "POST", ...json({ username, password }) }),
  logout: () => request<void>("/auth/logout", { method: "POST" }),
  getResume: () => request<MasterResume | null>("/resume"),
  uploadResume: (file: File) => {
    const form = new FormData();
    form.append("file", file);
    return request<MasterResume>("/resume", { method: "POST", body: form });
  },
  deleteResume: () => request<void>("/resume", { method: "DELETE" }),
  resumeFileUrl: (version: string) => `/api/resume/file?v=${encodeURIComponent(version)}`,
};

export type WorkMode = "onsite" | "hybrid" | "remote";
export type PostedWithin = "day" | "week" | "month";
export type Site = "linkedin" | "indeed";
export type JobStatus = "new" | "saved" | "dismissed" | "applied";

export interface SearchInput {
  name?: string;
  titles: string[];
  keywords: string[];
  location?: string;
  work_modes: WorkMode[];
  posted_within: PostedWithin;
}

export interface JobSearch {
  id: number;
  enabled: boolean;
  name: string;
  titles: string[];
  keywords: string[];
  location: string | null;
  work_modes: WorkMode[];
  posted_within: PostedWithin;
  created_at: string;
  updated_at: string;
}

export interface JobListing {
  id: number;
  source: Site;
  source_id: string;
  title: string;
  company: string;
  company_url: string | null;
  company_website: string | null;
  location: string;
  posted_on: string | null;
  url: string;
  description: string | null;
  seniority: string | null;
  employment_type: string | null;
  industries: string | null;
  work_mode: WorkMode | null;
  status: JobStatus;
  first_seen_at: string;
  tailored_at: string | null;
  search_ids: number[];
  matched_keywords: string[];
  score: JobScore | null;
}

export interface Requirement {
  text: string;
  importance: "must" | "nice";
  verdict: "met" | "partial" | "missing";
  evidence: string | null;
}

export interface JobScore {
  status: "scoring" | "ready" | "failed" | "skipped";
  score: number | null;
  summary: string | null;
  requirements: Requirement[];
  model: string;
  error: string | null;
  profile_id: number;
  updated_at: string;
}

export interface ScanRun {
  id: number;
  started_at: string;
  finished_at: string | null;
  status: "running" | "ok" | "partial" | "failed" | "interrupted";
  new_matches: number;
  message: string | null;
}

export interface ScanStatus {
  running: boolean;
  interval_minutes: number;
  next_run_at: string | null;
  last_run: ScanRun | null;
}

export interface Settings {
  scan_interval_minutes: number;
}

const json = (body: unknown): RequestInit => ({
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
});

export const jobsApi = {
  listSearches: () => request<JobSearch[]>("/searches"),
  createSearch: (input: SearchInput) => request<JobSearch>("/searches", { method: "POST", ...json(input) }),
  updateSearch: (id: number, input: SearchInput) => request<JobSearch>(`/searches/${id}`, { method: "PUT", ...json(input) }),
  setSearchEnabled: (id: number, enabled: boolean) => request<JobSearch>(`/searches/${id}`, { method: "PATCH", ...json({ enabled }) }),
  deleteSearch: (id: number) => request<void>(`/searches/${id}`, { method: "DELETE" }),
  listJobs: (status?: JobStatus) => request<JobListing[]>(status ? `/jobs?status=${status}` : "/jobs"),
  setJobStatus: (id: number, status: JobStatus) => request<void>(`/jobs/${id}`, { method: "PATCH", ...json({ status }) }),
  setJobsStatus: (ids: number[], status: JobStatus) =>
    request<{ updated: number }>("/jobs/status", { method: "POST", ...json({ ids, status }) }),
  rescore: (id: number) => request<void>(`/jobs/${id}/score`, { method: "POST" }),
  skipScore: (id: number) => request<void>(`/jobs/${id}/score`, { method: "DELETE" }),
  scanStatus: () => request<ScanStatus>("/scan"),
  runScan: () => request<void>("/scan/run", { method: "POST" }),
  getSettings: () => request<Settings>("/settings"),
  updateSettings: (settings: Settings) => request<Settings>("/settings", { method: "PUT", ...json(settings) }),
};

export interface Bullet {
  id: string;
  text: string;
}

export interface ResumeProfile {
  contact: { name: string; email: string | null; phone: string | null; location: string | null; links: string[] };
  summary: string | null;
  headline: string | null;
  experience: {
    id: string;
    company: string;
    title: string;
    location: string | null;
    start: string | null;
    end: string | null;
    bullets: Bullet[];
  }[];
  projects: { id: string; name: string; description: string | null; bullets: Bullet[] }[];
  education: { id: string; school: string; degree: string | null; field: string | null; start: string | null; end: string | null }[];
  skills: { name: string; years: number | null; used_at: string[] }[];
  certifications: string[];
  languages: string[];
  awards: string[];
}

export interface ProfileRecord {
  id: number;
  resume_uploaded_at: string;
  status: "reading" | "ready" | "failed" | "confirmed";
  model: string;
  profile: ResumeProfile | null;
  warnings: { item_id: string; message: string }[];
  error: string | null;
  created_at: string;
  updated_at: string;
  confirmed_at: string | null;
}

export interface AiStatus {
  ollama: { url: string; reachable: boolean; models: string[] };
  resume_reader_model: string;
  ollama_url: string;
}

export const profileApi = {
  get: () => request<ProfileRecord | null>("/resume/profile"),
  read: () => request<void>("/resume/profile/read", { method: "POST" }),
  save: (profile: ResumeProfile) => request<ProfileRecord>("/resume/profile", { method: "PUT", ...json(profile) }),
  confirm: () => request<ProfileRecord>("/resume/profile/confirm", { method: "POST" }),
  aiStatus: () => request<AiStatus>("/ai"),
  updateAiSettings: (settings: { resume_reader_model?: string; ollama_url?: string }) =>
    request<AiStatus>("/ai", { method: "PUT", ...json(settings) }),
};

export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : "Something went wrong.";
}

type Loadable<T> = { status: "loading" } | { status: "error"; error: string } | { status: "ready"; data: T };

export function useResume() {
  const [state, setState] = useState<Loadable<MasterResume | null>>({ status: "loading" });

  const reload = useCallback(async () => {
    try {
      setState({ status: "ready", data: await api.getResume() });
    } catch (e) {
      setState({ status: "error", error: errorMessage(e) });
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const setResume = useCallback((data: MasterResume | null) => setState({ status: "ready", data }), []);

  return { state, reload, setResume };
}
