import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, ExternalLink, FileText, Info, Loader2, RefreshCw, Sparkles, Trash2 } from "lucide-react";
import { MAX_UPLOAD_BYTES, api, errorMessage, profileApi, useResume, type MasterResume, type ProfileRecord, type ResumeProfile } from "../api";
import { Dropzone } from "../components/Dropzone";
import { JsonView } from "../components/JsonView";
import { ProfileEditor } from "../components/ProfileEditor";
import { useDevMode } from "../devMode";
import { Alert, Button, Card, PageHeader, Skeleton } from "../components/ui";
import { formatBytes, formatDate, plural } from "../format";

type Tab = "profile" | "preview" | "text" | "json";

function useProfile(resumeVersion: string) {
  const [record, setRecord] = useState<ProfileRecord | null | undefined>(undefined);
  const load = useCallback(async () => setRecord(await profileApi.get()), []);

  useEffect(() => {
    setRecord(undefined);
    void load();
  }, [load, resumeVersion]);

  useEffect(() => {
    if (record?.status !== "reading") return;
    const id = setInterval(() => void load(), 3000);
    return () => clearInterval(id);
  }, [record?.status, load]);

  return { record, setRecord, load };
}

function validateFile(file: File): string | null {
  const isPdf = file.type === "application/pdf" || file.name.toLowerCase().endsWith(".pdf");
  if (!isPdf) return "Please choose a PDF file.";
  if (file.size === 0) return "That file is empty.";
  if (file.size > MAX_UPLOAD_BYTES) return "That file is too large. The limit is 10 MB.";
  return null;
}

export function ResumePage() {
  const { state, setResume } = useResume();
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function upload(file: File) {
    const problem = validateFile(file);
    if (problem) {
      setError(problem);
      return;
    }
    setError(null);
    setUploading(true);
    try {
      setResume(await api.uploadResume(file));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setUploading(false);
    }
  }

  async function remove() {
    if (!confirm("Remove your master resume? You can upload a new one anytime.")) return;
    setError(null);
    try {
      await api.deleteResume();
      setResume(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  return (
    <>
      <PageHeader title="Master resume" description="The baseline every tailored version is built from." />
      {error && <Alert onDismiss={() => setError(null)}>{error}</Alert>}

      {state.status === "loading" && (
        <div className="space-y-4">
          <Skeleton className="h-24" />
          <Skeleton className="h-96" />
        </div>
      )}
      {state.status === "error" && <Alert>{state.error}</Alert>}
      {state.status === "ready" &&
        (state.data ? (
          <ResumeDetails resume={state.data} uploading={uploading} onReplace={upload} onRemove={remove} />
        ) : (
          <Dropzone onFile={upload} busy={uploading} />
        ))}
    </>
  );
}

function ResumeDetails({
  resume,
  uploading,
  onReplace,
  onRemove,
}: {
  resume: MasterResume;
  uploading: boolean;
  onReplace: (file: File) => void;
  onRemove: () => void;
}) {
  const [selectedTab, setTab] = useState<Tab>("profile");
  const [devMode] = useDevMode();
  const replaceInput = useRef<HTMLInputElement>(null);
  const fileUrl = api.resumeFileUrl(resume.uploaded_at);
  const { record, setRecord, load } = useProfile(resume.uploaded_at);
  const [error, setError] = useState<string | null>(null);

  async function readAgain() {
    if (record?.profile && !confirm("Read your resume again? Corrections you made to this reading will be replaced.")) return;
    setError(null);
    try {
      await profileApi.read();
      await load();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const tabs: [Tab, string][] = [
    ["profile", "What the AI understood"],
    ["preview", "Preview"],
    ["text", "Extracted text"],
    ...(devMode ? [["json", "JSON"] as [Tab, string]] : []),
  ];
  const tab = selectedTab === "json" && !devMode ? "preview" : selectedTab;

  return (
    <div className="space-y-6">
      <Card className="flex flex-wrap items-center gap-4 p-5">
        <div className="flex size-11 shrink-0 items-center justify-center rounded-lg bg-red-50">
          <FileText className="size-5 text-red-600" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium" title={resume.file_name}>
            {resume.file_name}
          </p>
          <p className="mt-0.5 text-sm text-zinc-500">
            {plural(resume.page_count, "page")} · {formatBytes(resume.size_bytes)} · Uploaded {formatDate(resume.uploaded_at)}
          </p>
        </div>
        <div className="grid w-full grid-cols-3 gap-2 sm:flex sm:w-auto">
          <a
            href={fileUrl}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center justify-center gap-2 rounded-lg border border-zinc-200 bg-white px-3.5 py-2 text-sm font-medium text-zinc-700 shadow-sm transition-colors hover:bg-zinc-50 hover:text-zinc-900"
          >
            <ExternalLink className="size-4" />
            Open
          </a>
          <Button icon={RefreshCw} loading={uploading} onClick={() => replaceInput.current?.click()}>
            Replace
          </Button>
          <Button variant="danger" icon={Trash2} onClick={onRemove} disabled={uploading}>
            Remove
          </Button>
          <input
            ref={replaceInput}
            type="file"
            accept="application/pdf,.pdf"
            className="hidden"
            onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (!file) return;
              if (validateFile(file) || confirm(`Replace "${resume.file_name}" with "${file.name}"?`)) onReplace(file);
            }}
          />
        </div>
      </Card>

      {error && <Alert onDismiss={() => setError(null)}>{error}</Alert>}
      <ReadingStatus record={record} onRead={readAgain} />

      <Card className="overflow-hidden">
        <div role="tablist" className="flex gap-1 overflow-x-auto border-b border-zinc-200 px-3 pt-3">
          {tabs.map(([id, label]) => (
            <button
              key={id}
              role="tab"
              aria-selected={tab === id}
              onClick={() => setTab(id)}
              className={`-mb-px shrink-0 border-b-2 px-3 pb-2.5 text-sm font-medium transition-colors ${
                tab === id ? "border-zinc-900 text-zinc-900" : "border-transparent text-zinc-500 hover:text-zinc-900"
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        {tab === "json" ? (
          <JsonView value={{ resume, reading: record ?? null }} fileName="resume.json" note="GET /api/resume and /api/resume/profile" />
        ) : tab === "profile" ? (
          record?.profile && (record.status === "ready" || record.status === "confirmed") ? (
            <ProfileEditor
              key={record.id}
              record={record as ProfileRecord & { profile: ResumeProfile }}
              onSave={async (profile) => setRecord(await profileApi.save(profile))}
              onConfirm={async () => setRecord(await profileApi.confirm())}
            />
          ) : (
            <p className="p-6 text-sm text-zinc-500">
              {record === undefined
                ? "Loading…"
                : record?.status === "reading"
                  ? "The AI is reading your resume. Its results will show up here."
                  : "Nothing to show yet. Use “Read with AI” above."}
            </p>
          )
        ) : tab === "preview" ? (
          <>
            <div className="flex flex-col items-center px-6 py-12 text-center md:hidden">
              <FileText className="mb-3 size-8 text-zinc-400" />
              <p className="text-sm text-zinc-500">PDF previews don't display well on small screens.</p>
              <a
                href={fileUrl}
                target="_blank"
                rel="noreferrer"
                className="mt-4 inline-flex items-center gap-2 rounded-lg bg-zinc-900 px-3.5 py-2 text-sm font-medium text-white shadow-sm hover:bg-zinc-800"
              >
                <ExternalLink className="size-4" />
                Open PDF
              </a>
            </div>
            <iframe key={fileUrl} src={fileUrl} title="Resume preview" className="hidden h-[75vh] w-full bg-zinc-100 md:block" />
          </>
        ) : resume.extracted_text ? (
          <div className="p-6">
            <p className="mb-4 text-xs text-zinc-400">
              {resume.extracted_text.length.toLocaleString()} characters · this raw text is what the AI will read
            </p>
            <pre className="font-sans text-sm leading-relaxed whitespace-pre-wrap text-zinc-700">{resume.extracted_text}</pre>
          </div>
        ) : (
          <div className="p-6 text-sm text-zinc-500">
            No text could be extracted. This usually means the PDF is a scanned image. Try exporting your resume to PDF directly from
            Word or Google Docs instead.
          </div>
        )}
      </Card>
    </div>
  );
}

function ReadingStatus({ record, onRead }: { record: ProfileRecord | null | undefined; onRead: () => void }) {
  if (record === undefined) return null;

  const box = "flex flex-wrap items-center gap-x-4 gap-y-2 rounded-lg border px-4 py-3 text-sm";
  const readButton = (label: string) => (
    <Button icon={Sparkles} onClick={onRead} className="ml-auto">
      {label}
    </Button>
  );

  if (record === null) {
    return (
      <div className={`${box} border-zinc-200 bg-zinc-100/60 text-zinc-600`}>
        <Info className="size-4 shrink-0" />
        <p className="flex-1">This resume hasn't been read by the AI yet.</p>
        {readButton("Read with AI")}
      </div>
    );
  }
  if (record.status === "reading") {
    return (
      <div className={`${box} border-sky-200 bg-sky-50 text-sky-800`}>
        <Loader2 className="size-4 shrink-0 animate-spin" />
        <p className="flex-1">Reading your resume with {record.model}. This usually takes under a minute.</p>
      </div>
    );
  }
  if (record.status === "failed") {
    return (
      <div className={`${box} border-red-200 bg-red-50 text-red-800`}>
        <AlertTriangle className="size-4 shrink-0" />
        <p className="flex-1">Reading failed: {record.error}</p>
        {readButton("Try again")}
      </div>
    );
  }
  const flagged = record.warnings.length;
  return (
    <div className={`${box} ${record.status === "confirmed" ? "border-emerald-200 bg-emerald-50 text-emerald-800" : "border-amber-200 bg-amber-50 text-amber-900"}`}>
      {record.status === "confirmed" ? <CheckCircle2 className="size-4 shrink-0" /> : <Info className="size-4 shrink-0" />}
      <p className="flex-1">
        {record.status === "confirmed"
          ? `Confirmed ${formatDate(record.confirmed_at!)} · read by ${record.model}`
          : `Read by ${record.model}. Check it below, fix anything wrong, then confirm.`}
        {flagged > 0 && record.status !== "confirmed" && ` ${plural(flagged, "bullet")} to double-check.`}
      </p>
      {readButton("Read again")}
    </div>
  );
}
