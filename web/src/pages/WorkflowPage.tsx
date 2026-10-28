import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MarkerType,
  Panel,
  Position,
  ReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  Brain,
  Check,
  FileDown,
  Filter,
  Gauge,
  PenLine,
  ScanText,
  Search,
  Upload,
  Workflow,
  X,
} from "lucide-react";
import { errorMessage, jobsApi, profileApi, workflowApi, type AiStep, type AiStepConfig, type Settings } from "../api";
import { Dialog } from "../components/Dialog";
import { Alert, Button, ButtonLink } from "../components/ui";

type StepId = "find_jobs" | "match_jobs" | "score_jobs" | "tailor_resume" | "make_pdf" | "upload_resume" | "read_resume" | "brain";
type StepKind = "plain" | "ai" | "planned";
type StepNodeData = { id?: StepId; title?: string; subtitle?: string; kind?: StepKind; model?: string; selected?: boolean; onHover?: (hovered: boolean) => void; label?: string };

const steps: { id: StepId; title: string; subtitle: string; kind: StepKind; x: number; y: number; icon: typeof Search }[] = [
  { id: "find_jobs", title: "Find jobs", subtitle: "LinkedIn and Indeed", kind: "plain", x: 0, y: 0, icon: Search },
  { id: "match_jobs", title: "Match jobs", subtitle: "Titles and keywords", kind: "plain", x: 280, y: 0, icon: Filter },
  { id: "score_jobs", title: "Score jobs", subtitle: "AI", kind: "ai", x: 560, y: 0, icon: Gauge },
  { id: "tailor_resume", title: "Tailor resume", subtitle: "Not built yet", kind: "planned", x: 840, y: 0, icon: PenLine },
  { id: "make_pdf", title: "Make PDF", subtitle: "Not built yet", kind: "planned", x: 1120, y: 0, icon: FileDown },
  { id: "upload_resume", title: "Upload resume", subtitle: "PDF to text", kind: "plain", x: 280, y: 220, icon: Upload },
  { id: "read_resume", title: "Read resume", subtitle: "AI", kind: "ai", x: 560, y: 220, icon: ScanText },
  { id: "brain", title: "Brain", subtitle: "Not built yet", kind: "planned", x: 840, y: 220, icon: Brain },
];

const aiFor: Partial<Record<StepId, AiStep>> = { read_resume: "read_resume", score_jobs: "score_jobs" };
const stepById = Object.fromEntries(steps.map((step) => [step.id, step])) as Record<StepId, (typeof steps)[number]>;

function StepNode({ data }: NodeProps<Node<StepNodeData>>) {
  if (!data.id) return <span className="text-xs font-semibold text-zinc-500">{data.label}</span>;
  const Icon = stepById[data.id].icon;
  const planned = data.kind === "planned";
  const ai = data.kind === "ai";
  return (
    <div
      aria-disabled={planned ? "true" : undefined}
      onMouseEnter={() => data.onHover?.(true)}
      onMouseLeave={() => data.onHover?.(false)}
      className={`relative flex h-[76px] w-[220px] items-center gap-3 rounded-xl border px-3 shadow-sm ${planned ? "cursor-not-allowed border-dashed border-zinc-300 bg-white opacity-50" : ai ? "cursor-pointer border-violet-200 border-l-4 border-l-violet-500 bg-violet-50" : "cursor-pointer border-zinc-200 bg-white"} ${data.selected ? "ring-2 ring-zinc-900" : ""}`}
    >
      <Handle type="target" position={Position.Left} className="!size-2 !border-0 !bg-zinc-400 !opacity-0" />
      <Handle type="source" position={Position.Right} className="!size-2 !border-0 !bg-zinc-400 !opacity-0" />
      <Handle type="target" position={Position.Top} id="top-target" className="!size-2 !border-0 !bg-zinc-400 !opacity-0" />
      <Handle type="source" position={Position.Bottom} id="bottom-source" className="!size-2 !border-0 !bg-zinc-400 !opacity-0" />
      <span className={`flex size-10 shrink-0 items-center justify-center rounded-lg ${ai ? "bg-violet-100 text-violet-700" : planned ? "bg-zinc-100 text-zinc-400" : "bg-zinc-100 text-zinc-700"}`}>
        <Icon className="size-5" />
      </span>
      <span className="min-w-0">
        <span className="block truncate text-sm font-semibold text-zinc-900">{data.title}</span>
        <span className={`mt-0.5 block truncate text-xs ${ai ? "text-violet-700" : "text-zinc-500"}`}>{ai ? data.model || "Loading model…" : data.subtitle}</span>
      </span>
      {ai && <span className="absolute right-2 top-2 rounded-full bg-violet-100 px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wide text-violet-700">AI</span>}
    </div>
  );
}

const nodeTypes = { step: StepNode, lane: StepNode };

export function WorkflowPage() {
  const [params, setParams] = useSearchParams();
  const [phoneWidth, setPhoneWidth] = useState(() => window.matchMedia("(max-width: 639px)").matches);
  const [pointerOverNode, setPointerOverNode] = useState(false);
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

  useEffect(() => {
    const media = window.matchMedia("(max-width: 639px)");
    const update = () => setPhoneWidth(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  const current = selected ? stepById[selected] : undefined;
  const aiStep = current ? aiFor[current.id] : undefined;
  const config = aiStep ? configs.find((item) => item.step === aiStep) : undefined;
  const dirty = Boolean(config && promptDraft !== config.prompt);

  useEffect(() => {
    if (config) setPromptDraft(config.prompt);
  }, [config?.step, config?.prompt]);

  const changeStep = useCallback((id: StepId | null) => {
    const next = new URLSearchParams(params);
    if (id) next.set("step", id);
    else next.delete("step");
    setParams(next);
  }, [params, setParams]);

  const requestSelect = useCallback((id: StepId) => {
    if (dirty) {
      setPendingStep(id);
      return;
    }
    changeStep(id);
  }, [changeStep, dirty]);

  const onPaneClick = useCallback(() => {
    if (!selected) return;
    if (dirty) setPendingStep(null);
    else changeStep(null);
  }, [changeStep, dirty, selected]);

  const onNodeClick = useCallback((_event: React.MouseEvent, node: Node<StepNodeData>) => {
    const id = node.data.id;
    if (id && stepById[id].kind !== "planned") requestSelect(id);
  }, [requestSelect]);

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

  const nodes = useMemo<Node<StepNodeData>[]>(() => [
    { id: "lane-jobs", type: "lane", position: { x: -90, y: 24 }, data: { label: "Jobs" }, draggable: false, selectable: false, style: { border: 0, background: "transparent", padding: 0, boxShadow: "none" } },
    { id: "lane-resume", type: "lane", position: { x: 180, y: 244 }, data: { label: "Resume" }, draggable: false, selectable: false, style: { border: 0, background: "transparent", padding: 0, boxShadow: "none" } },
    ...steps.map((step) => {
      const stepConfig = aiFor[step.id] ? configs.find((item) => item.step === aiFor[step.id]) : undefined;
      return {
        id: step.id,
        type: "step",
        position: { x: step.x, y: step.y },
        data: { id: step.id, title: step.title, subtitle: step.subtitle, kind: step.kind, model: stepConfig?.model, selected: selected === step.id, onHover: setPointerOverNode },
        draggable: false,
        selectable: false,
      };
    }),
  ], [configs, selected]);

  const edges = useMemo<Edge[]>(() => {
    const links: [StepId, StepId, boolean?][] = [
      ["find_jobs", "match_jobs"],
      ["match_jobs", "score_jobs"],
      ["score_jobs", "tailor_resume"],
      ["tailor_resume", "make_pdf"],
      ["upload_resume", "read_resume"],
      ["read_resume", "brain"],
      ["read_resume", "score_jobs", true],
      ["brain", "tailor_resume", true],
    ];
    return links.map(([source, target, crossLane]) => {
      const planned = stepById[source].kind === "planned" || stepById[target].kind === "planned";
      const vertical = crossLane || (stepById[source].x === stepById[target].x && stepById[source].y !== stepById[target].y);
      return {
        id: `${source}-${target}`,
        source,
        target,
        sourceHandle: vertical ? "bottom-source" : undefined,
        targetHandle: vertical ? "top-target" : undefined,
        type: "smoothstep",
        animated: Boolean(crossLane),
        markerEnd: { type: MarkerType.ArrowClosed, color: planned ? "#d4d4d8" : "#a1a1aa", width: 16, height: 16 },
        style: { stroke: planned ? "#d4d4d8" : "#a1a1aa", strokeWidth: 1.5, ...(planned ? { strokeDasharray: "6 4" } : {}) },
      };
    });
  }, []);

  return (
    <div className="-mx-4 -mt-6 flex h-[calc(100dvh-4rem)] min-h-[520px] flex-col overflow-hidden sm:-mx-6 lg:-mx-8">
      <header className="z-10 flex shrink-0 items-center justify-between gap-4 border-b border-zinc-200 bg-white px-4 py-3 sm:px-6">
        <div className="flex min-w-0 items-center gap-3">
          <Workflow className="size-5 shrink-0 text-zinc-700" />
          <div className="min-w-0"><h1 className="text-base font-semibold text-zinc-900">Workflow</h1><p className="truncate text-xs text-zinc-500 sm:text-sm">How the app finds jobs and uses your resume. Click a step to change it.</p></div>
        </div>
      </header>
      {error && <div className="shrink-0 px-4 pt-2 sm:px-6"><Alert onDismiss={() => setError(null)}>{error}</Alert></div>}
      <div className="relative min-h-0 flex-1">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          fitView
          fitViewOptions={{ padding: phoneWidth ? 0.04 : 0.24, minZoom: phoneWidth ? 0.12 : 0.35, maxZoom: 1 }}
          minZoom={0.25}
          maxZoom={1.5}
          nodesDraggable={false}
          nodesConnectable={false}
          elementsSelectable={false}
          zoomOnScroll
          zoomOnPinch
          proOptions={{ hideAttribution: true }}
          onPaneClick={onPaneClick}
          onNodeClick={onNodeClick}
          panOnDrag={!pointerOverNode}
          className="bg-zinc-50"
        >
          <Background variant={BackgroundVariant.Dots} gap={20} size={1.5} color="#e4e4e7" />
          <Controls showInteractive={false} position="bottom-left" />
          <Panel position="top-left" className="m-3 rounded-xl border border-zinc-200 bg-white/95 p-3 shadow-sm">
            <div className="space-y-2 text-xs text-zinc-600">
              <div className="flex items-center gap-2"><span className="size-3 rounded border border-violet-200 bg-violet-50" />AI step</div>
              <div className="flex items-center gap-2"><span className="size-3 rounded border border-zinc-300 bg-white" />Runs without AI</div>
              <div className="flex items-center gap-2"><span className="size-3 rounded border border-dashed border-zinc-300 opacity-60" />Not built yet</div>
            </div>
          </Panel>
        </ReactFlow>
        {current && <>
          <div className="fixed inset-0 z-20 hidden bg-black/10 lg:block" onClick={onPaneClick} />
          <aside className="absolute inset-x-0 bottom-0 z-30 h-[70%] overflow-y-auto rounded-t-2xl border-t border-zinc-200 bg-white p-5 shadow-2xl lg:fixed lg:inset-y-0 lg:right-0 lg:left-auto lg:h-full lg:w-[420px] lg:rounded-none lg:border-l lg:border-t-0 lg:p-6">
            <div className="mb-5 flex items-start justify-between gap-4">
              <div><h2 className="text-lg font-semibold text-zinc-900">{current.title}</h2><p className="mt-1 text-sm text-zinc-500">{current.subtitle}</p></div>
              <button type="button" onClick={() => dirty ? setPendingStep(null) : changeStep(null)} className="rounded-lg p-2 text-zinc-500 hover:bg-zinc-100 hover:text-zinc-900" aria-label="Close settings"><X className="size-5" /></button>
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
          </aside>
        </>}
      </div>
      {pendingStep !== null && <Dialog title="Discard your prompt changes?" onClose={() => setPendingStep(null)} actions={<><Button onClick={() => setPendingStep(null)}>Keep editing</Button><Button variant="primary" onClick={() => { changeStep(pendingStep); setPendingStep(null); }}>Discard</Button></>}><p>Your unsaved prompt changes will be lost.</p></Dialog>}
      {confirmReset && <Dialog title="Reset to default?" onClose={() => setConfirmReset(false)} actions={<><Button onClick={() => setConfirmReset(false)}>Cancel</Button><Button variant="primary" onClick={resetPrompt}>Reset to default</Button></>}><p>This replaces the saved prompt with the original prompt.</p></Dialog>}
    </div>
  );
}
