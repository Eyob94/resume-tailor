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
      // Non-JSON error body; keep the fallback message.
    }
    throw new ApiError(message, res.status);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

export const api = {
  health: () => request<{ ok: boolean }>("/health"),
  getResume: () => request<MasterResume | null>("/resume"),
  uploadResume: (file: File) => {
    const form = new FormData();
    form.append("file", file);
    return request<MasterResume>("/resume", { method: "POST", body: form });
  },
  deleteResume: () => request<void>("/resume", { method: "DELETE" }),
  /** `version` busts the browser cache after a replace. */
  resumeFileUrl: (version: string) => `/api/resume/file?v=${encodeURIComponent(version)}`,
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
