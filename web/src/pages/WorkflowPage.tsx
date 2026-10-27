import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Check, ChevronDown, ChevronRight } from "lucide-react";
import { errorMessage, jobsApi, profileApi, workflowApi, type AiStep, type AiStepConfig, type Settings } from "../api";
import { Dialog } from "../components/Dialog";
import { Alert, Button, ButtonLink, Card, PageHeader } from "../components/ui";

type StepId = "find_jobs" | "match_jobs" | "score_jobs" | "tailor_resume" | "make_pdf" | "upload_resume" | "read_resume" | "brain";
type Step = { id: StepId; title: string; subtitle: string; lane: "Jobs" | "Resume"; kind: "plain" | "ai" | "planned" };

const steps: Step[] = [
  { id: "find_jobs", title: "Find jobs", subtitle: "LinkedIn and Indeed", lane: "Jobs", kind: "plain" },
  { id: "match_jobs", title: "Match jobs", subtitle: "Titles and keywords", lane: "Jobs", kind: "plain" },
  { id: "score_jobs", title: "Score jobs", subtitle: "AI", lane: "Jobs", kind: "ai" },
  { id: "tailor_resume", title: "Tailor resume", subtitle: "Not built yet", lane: "Jobs", kind: "planned" },
  { id: "make_pdf", title: "Make PDF", subtitle: "Not built yet", lane: "Jobs", kind: "planned" },
  { id: "upload_resume", title: "Upload resume", subtitle: "PDF to text", lane: "Resume", kind: "plain" },
  { id: "read_resume", title: "Read resume", subtitle: "AI", lane: "Resume", kind: "ai" },
  { id: "brain", title: "Brain", subtitle: "Not built yet", lane: "Resume", kind: "planned" },
];

const aiFor: Partial<Record<StepId, AiStep>> = { read_resume: "read_resume", score_jobs: "score_jobs" };

export function WorkflowPage() {
  const [params, setParams] = useSearchParams();
  const selected = params.get("step") as StepId | null;
  const [configs, setConfigs] = useState<AiStepConfig[]>([]);
  const [aiReachable, setAiReachable] = useState(false);
  const [ollamaModels, setOllamaModels] = useState<string[]>([]);
  const [intervals, setIntervals] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [promptDraft, setPromptDraft] = useState("");
  const [pendingStep, setPendingStep] = useState<StepId | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const [savingPrompt, setSavingPrompt] = useState(false);

  useEffect(() => {
    workflowApi.aiSteps().then(setConfigs, (e) => setError(errorMessage(e)));
    profileApi.aiStatus().then((status) => { setAiReachable(status.ollama.reachable); setOllamaModels(status.ollama.models); }, (e) => setError(errorMessage(e)));
    jobsApi.getSettings().then(setIntervals, (e) => setError(errorMessage(e)));
  }, []);

  const current = steps.find((step) => step.id === selected);
  const aiStep = current ? aiFor[current.id] : undefined;
  const config = aiStep ? configs.find((item) => item.step === aiStep) : undefined;
  const dirty = Boolean(config && promptDraft !== config.prompt);

  useEffect(() => {
    if (config) setPromptDraft(config.prompt);
  }, [config?.step, config?.prompt]);

  const changeStep = (id: StepId | null) => {
    const next = new URLSearchParams(params);
    if (id) next.set("step", id);
    else next.delete("step");
    setParams(next);
  };

  const requestSelect = (id: StepId) => {
    if (dirty) {
      setPendingStep(id);
      return;
    }
    changeStep(id);
  };

  async function saveModel(step: AiStep, model: string) {
    try {
      const updated = await workflowApi.updateAiStep(step, { model });
      setConfigs((all) => all.map((item) => item.step === step ? updated : item));
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function savePrompt() {
    if (!aiStep) return;
    setSavingPrompt(true);
    try {
      const updated = await workflowApi.updateAiStep(aiStep, { prompt: promptDraft });
      setConfigs((all) => all.map((item) => item.step === aiStep ? updated : item));
      setPromptDraft(updated.prompt);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSavingPrompt(false);
    }
  }

  async function resetPrompt() {
    if (!aiStep) return;
    try {
      const updated = await workflowApi.resetPrompt(aiStep);
      setConfigs((all) => all.map((item) => item.step === aiStep ? updated : item));
      setPromptDraft(updated.prompt);
      setConfirmReset(false);
    } catch (e) {
      setError(errorMessage(e));
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

  function lane(name: "Jobs" | "Resume") {
    const laneSteps = steps.filter((step) => step.lane === name);
    return (
      <section aria-label={`${name} workflow`} className="mb-5">
        <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-zinc-500">{name}</h2>
        <div className="overflow-x-auto pb-1">
          <div className="flex min-w-0 flex-col gap-2 md:min-w-max md:flex-row md:items-stretch md:gap-0">
          {laneSteps.map((step, index) => {
            const stepConfig = aiFor[step.id] ? configs.find((item) => item.step === aiFor[step.id]) : undefined;
            const selectedStyle = selected === step.id ? "ring-2 ring-zinc-900" : "";
            const style = step.kind === "ai" ? "border-violet-200 bg-violet-50" : step.kind === "planned" ? "border-dashed border-zinc-300 opacity-50" : "border-zinc-200 bg-white";
            return (
              <div key={step.id} className="flex flex-col gap-2 md:flex-row md:items-center md:gap-1">
                {index > 0 && <ChevronDown className="mx-auto size-4 text-zinc-400 md:hidden" />}
                {index > 0 && <ChevronRight className="hidden size-4 shrink-0 text-zinc-400 md:block" />}
                <button
                  type="button"
                  aria-disabled={step.kind === "planned" ? "true" : undefined}
                  disabled={step.kind === "planned"}
                  onClick={() => requestSelect(step.id)}
                  className={`min-w-0 flex-1 rounded-xl border p-3 text-left ${style} ${selectedStyle} ${step.kind === "planned" ? "cursor-not-allowed" : "transition-shadow hover:shadow-sm"} md:w-[104px] md:flex-none`}
                >
                  <span className="flex items-center justify-between gap-2">
                    <span className="truncate text-xs font-medium text-zinc-900 md:text-[11px]">{step.title}</span>
                  </span>
                  <span className="mt-1 block truncate text-xs text-zinc-500">{step.kind === "ai" ? <span className="rounded-full bg-violet-100 px-2 py-0.5 text-[10px] font-semibold text-violet-800">{step.subtitle}</span> : step.kind === "planned" ? <span className="rounded-full bg-zinc-100 px-2 py-0.5 text-[10px] text-zinc-500">{step.subtitle}</span> : step.subtitle}</span>
                  {stepConfig && <span className="mt-2 block truncate text-xs font-medium text-violet-800">{stepConfig.model}</span>}
                  {step.id === "score_jobs" && <span className="mt-2 inline-block rounded bg-white/80 px-2 py-1 text-[10px] text-zinc-600">Uses: Read resume</span>}
                  {step.id === "tailor_resume" && <span className="mt-2 inline-block rounded bg-white/80 px-2 py-1 text-[10px] text-zinc-600">Uses: Brain</span>}
                </button>
              </div>
            );
          })}
          </div>
        </div>
      </section>
    );
  }

  return (
    <>
      <PageHeader title="Workflow" description="How the app finds jobs and uses your resume. Click a step to change it." />
      {error && <Alert onDismiss={() => setError(null)}>{error}</Alert>}
      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_400px]">
        <div>
          {lane("Jobs")}
          {lane("Resume")}
          <div className="flex flex-wrap gap-x-5 gap-y-2 pt-1 text-xs text-zinc-500">
            <span className="inline-flex items-center gap-1.5"><span className="size-3 rounded border border-violet-200 bg-violet-50" />AI step</span>
            <span className="inline-flex items-center gap-1.5"><span className="size-3 rounded border border-zinc-300 bg-white" />Runs without AI</span>
            <span className="inline-flex items-center gap-1.5"><span className="size-3 rounded border border-dashed border-zinc-300 opacity-60" />Not built yet</span>
          </div>
        </div>
        <Card className="min-h-64 p-5">
          {!current ? <p className="text-sm text-zinc-500">Pick a step to see its settings.</p> : <>
            <div className="mb-4 flex items-start justify-between gap-3">
              <div><h2 className="font-semibold">{current.title}</h2><p className="mt-1 text-sm text-zinc-500">{current.subtitle}</p></div>
              <button type="button" onClick={() => dirty ? setPendingStep(null) : changeStep(null)} className="text-xs text-zinc-400 hover:text-zinc-700" aria-label="Clear selection">Clear</button>
            </div>
            {current.id === "find_jobs" && <div className="space-y-4 text-sm">
              <p>Finds jobs from enabled preferences on LinkedIn and Indeed.</p>
              {([ ["scan_interval_minutes", "LinkedIn"], ["indeed_scan_interval_minutes", "Indeed"] ] as [keyof Settings, string][]).map(([key, site]) => <label key={key} className="flex items-center justify-between gap-3"><span>{site} scan interval</span><select aria-label={`${site} scan interval`} value={intervals?.[key] ?? ""} disabled={!intervals} onChange={(e) => changeInterval(key, Number(e.target.value))} className="rounded-lg border border-zinc-200 bg-white px-3 py-2 text-sm">{[1, 5, 10, 15, 20, 30, 45, 60].map((m) => <option key={m} value={m}>Every {m === 60 ? "hour" : `${m} min`}</option>)}</select></label>)}
              <ButtonLink to="/jobs">Open Jobs</ButtonLink>
            </div>}
            {current.id === "match_jobs" && <ul className="list-disc space-y-2 pl-5 text-sm text-zinc-600"><li>Titles must match.</li><li>Keywords must appear in the title or description.</li><li>Work mode is read from the posting.</li></ul>}
            {current.id === "upload_resume" && <div className="space-y-4 text-sm"><p>Upload a PDF and the app extracts its text.</p><ButtonLink to="/resume">Open Master resume</ButtonLink></div>}
            {aiStep && config && <div className="space-y-5">
              <label className="block text-sm font-medium">Model<select aria-label="AI model" value={config.model} onChange={(e) => saveModel(aiStep, e.target.value)} className="mt-1.5 w-full rounded-lg border border-zinc-200 bg-white px-3 py-2 font-normal">{Array.from(new Set([config.model, ...ollamaModels])).map((model) => <option key={model} value={model}>{model}</option>)}</select></label>
              {!aiReachable && <p className="text-xs text-amber-700">Ollama not running</p>}
              <label className="block text-sm font-medium">System prompt<textarea rows={16} value={promptDraft} onChange={(e) => setPromptDraft(e.target.value)} className="mt-1.5 w-full resize-y rounded-lg border border-zinc-200 bg-white p-3 font-mono text-xs leading-5" /></label>
              <div className="flex items-center justify-between text-xs text-zinc-500"><span>{promptDraft.length.toLocaleString()} characters</span>{!config.prompt_is_default && <button type="button" onClick={() => setConfirmReset(true)} className="underline underline-offset-2">Reset to default</button>}</div>
              <Button variant="primary" icon={Check} loading={savingPrompt} disabled={!dirty} onClick={savePrompt}>Save prompt</Button>
              <div><h3 className="mb-1 text-xs font-semibold text-zinc-700">What this step receives</h3><pre className="overflow-x-auto whitespace-pre-wrap rounded-lg bg-zinc-50 p-3 font-mono text-xs text-zinc-600">{config.input_template}</pre></div>
              <p className="text-xs text-zinc-500">The output format is fixed so the app can read the answer.</p>
            </div>}
          </>}
        </Card>
      </div>
      {pendingStep !== null && <Dialog title="Discard your prompt changes?" onClose={() => setPendingStep(null)} actions={<><Button onClick={() => setPendingStep(null)}>Keep editing</Button><Button variant="primary" onClick={() => { changeStep(pendingStep); setPendingStep(null); }}>Discard</Button></>}><p>Your unsaved prompt changes will be lost.</p></Dialog>}
      {confirmReset && <Dialog title="Reset to default?" onClose={() => setConfirmReset(false)} actions={<><Button onClick={() => setConfirmReset(false)}>Cancel</Button><Button variant="primary" onClick={resetPrompt}>Reset to default</Button></>}><p>This replaces the saved prompt with the original prompt.</p></Dialog>}
    </>
  );
}
