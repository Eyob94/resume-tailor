# Resume Tailor — Spec v0.3

Personal, single-user web app. Upload a resume PDF once → parse to structured JSON → paste a job description → Claude tailors the resume under configurable rules → review changes → export a plain, ATS-friendly PDF.

**Decisions locked**
- Claude only
- Web app: TypeScript frontend + Rust backend
- PDF upload as baseline
- One master resume
- Configurable tailoring and length
- One plain standard template (no photo, no columns, no graphics)
- No cover letters
- Nothing built "for later": v1 scope only

---

## 1. Architecture

```
Browser ── React + Vite + TS ──┐
                               │ JSON over HTTP (localhost)
Rust server (Axum) ────────────┘
  core::schema    Resume / JobAnalysis / TailoringPlan types
  core::claude    Messages API client (reqwest, raw HTTP)
  core::pipeline  ingest → analyze → tailor → verify → apply
  core::verify    deterministic grounding checks
  core::render    Typst → PDF
  core::store     SQLite + files on disk
```

- `crates/core`: a plain library holding all the logic, with no web framework in it.
- `crates/server`: the Axum app, a thin layer of HTTP handlers over `core`. In production it also serves the built frontend as static files, so there's one process to run.
- `crates/cli`: a dev harness (`ingest`, `analyze`, `tailor`, `render`) for iterating on prompts and templates without the UI.
- `web/`: React + Vite + TypeScript, Tailwind, TanStack Query. In dev, Vite proxies `/api` to the Rust server.

Runs on `localhost`. There's no auth in v1 (single user, local machine).

### Crates
| Purpose | Crate |
|---|---|
| Web server | `axum`, `tokio`, `tower-http` (static files, CORS in dev) |
| HTTP to Claude | `reqwest` (rustls) + SSE parsing |
| Types ↔ JSON Schema | `serde`, `serde_json`, `schemars` |
| TS types from Rust | `ts-rs` |
| PDF render | `typst` + `typst-pdf` (embedded via `typst-as-lib`) |
| Storage | `rusqlite` (bundled) |
| Config / errors / logs | `dotenvy`, `thiserror`, `tracing` |

**API key:** `ANTHROPIC_API_KEY` is read from the environment or a `.env` file (gitignored). It never leaves the server. The settings page shows connection status only.

---

## 2. Data model

Every resume item gets a stable ID. That ID is what the guardrails, diffs and traceability hang off.

```rust
struct Resume {
    contact: Contact,                 // name, email, phone, location, links[]
    summary: Option<Item>,            // Item = { id, text }
    experience: Vec<Role>,
    projects: Vec<Project>,
    education: Vec<Education>,
    skills: Vec<SkillGroup>,          // { id, label, skills: Vec<Item> }
    certifications: Vec<Item>,
    extra_sections: Vec<Section>,     // awards, publications, volunteering, ...
}

struct Role {
    id: String,
    company: String, title: String, location: Option<String>,
    start: PartialDate, end: Option<PartialDate>,   // None = present
    bullets: Vec<Item>,
}
```

**Immutable facts:** contact info, company, title, dates, school, degree and certification names. None of them appear in the tailoring output schema, so the model has no way to change them.

```rust
struct JobAnalysis {
    company: Option<String>, title: String, seniority: Seniority,
    must_have: Vec<Requirement>,      // { text, keywords[] }
    nice_to_have: Vec<Requirement>,
    keywords: Vec<Keyword>,           // { term, category: Skill|Tool|Domain|Soft, weight: 1-3 }
    responsibilities: Vec<String>,
}

struct TailoringPlan {
    summary: Option<TextOp>,
    section_order: Vec<SectionKind>,
    roles: Vec<RolePlan>,             // { role_id, include, bullet_ops: Vec<BulletOp> }
    projects: Vec<ProjectPlan>,
    skills: Vec<SkillOp>,             // reorder / select existing ids only
    notes: Vec<String>,               // gaps & caveats ("JD wants Kubernetes; not in resume")
}

struct BulletOp {
    source_id: String,                // MUST reference an existing bullet
    action: Keep | Rewrite | Drop,
    text: Option<String>,             // required for Rewrite
    relevance: u8,                    // 1-5, used to trim when over page limit
    keywords_used: Vec<String>,
    rationale: String,                // shown in review
}
```

The model can **select, reorder, rephrase and drop**. It can't create anything: there is no way to express a bullet without a `source_id`.

---

## 3. Pipeline

Each step is one Claude call that returns schema-validated JSON. Each step's output is saved, so any step can be re-run on its own.

### Step 1: Ingest (once)
- The PDF is sent as a base64 `document` block, and Claude reads it directly. There's no local text extraction.
- Output is `Resume` JSON. The server assigns the IDs.
- The user corrects any parsing mistakes in a form editor, then saves it as **the** master resume. Uploading again replaces it, after a confirmation.

### Step 2: Analyze the job description
- Input is the pasted JD text. Output is `JobAnalysis`.
- It shows up as editable keyword chips. The user can delete noise or change weights before tailoring.

### Step 3: Tailor
- Input is the master resume (as a cached prompt prefix) + `JobAnalysis` + `TailoringConfig`.
- Output is a `TailoringPlan`.

### Step 4: Verify (deterministic Rust, no LLM)
**Hard failures.** The offending op is rejected, and the model is re-asked once with the error list.
- The `source_id` doesn't exist, or a locked item was modified.
- **Number guard:** every number, %, $ or time span in a rewrite must already appear in the source bullet.
- **Entity guard:** every tool, technology or proper noun in a rewrite must appear in the source bullet. If it appears elsewhere in the resume instead, that's a warning, not a failure.
- **Intensity guard:** in Light mode, a rewrite can't change more than about 30% of the bullet's tokens.

**Warnings** (shown during review):
- A JD must-have isn't covered anywhere. This is reported as a gap and never filled in.
- A keyword is repeated more than N times across the resume.

### Step 5: Review
- Each bullet is shown side by side: the source, the proposed text, the rationale, and the matched keywords highlighted.
- Every op can be accepted, rejected or edited inline. There's also an "accept all".
- A coverage meter shows must-haves covered X/Y and the weighted keyword score.
- A **Preview** button renders the PDF and shows it in an `<iframe>`.

### Step 6: Fit and export
- The accepted ops are applied, then the result is rendered and its pages are counted.
- If it runs past `max_pages`, the server drops accepted bullets in order of lowest `relevance` until it fits. The user can see which bullets were dropped and undo that.
- Download as `{Name}_{Company}_{Title}.pdf`.

### Step 7: Save
Each application record stores the JD, analysis, config, plan, decisions, final JSON, PDF and status.

---

## 4. Tailoring config

Top-level options:
- **Preset:** Light, Moderate or Aggressive
- **Max pages:** 1, 2 or 3 (configurable)
- **Model**
- **Effort**

| Preset | Light | Moderate | Aggressive |
|---|---|---|---|
| Bullet rewrites | keyword swaps only | rephrase for emphasis | full rewrite (facts preserved) |
| Summary | keep | light edit | rewrite to JD |
| Drop bullets | no | low-relevance | yes |
| Drop whole roles/projects | no | no | yes |
| Reorder bullets in a role | yes | yes | yes |
| Reorder sections | no | yes | yes |
| Skills | reorder | reorder + select | reorder + select |

**Advanced** (the preset fills these in, and each can be overridden):
- `max_bullets_per_role`
- `keyword_density`: natural, balanced or dense
- `tone`: neutral, technical or leadership
- `locked_ids`: set with a lock icon in the resume editor
- `include_roles_since`: a year

The config goes into the tailoring prompt as explicit rules. The verifier in Step 4 enforces the rules it can check.

---

## 5. Claude integration (raw HTTP from Rust)

There's no official Rust SDK, so `core::claude` is a thin typed client over `POST https://api.anthropic.com/v1/messages`.

**Request basics**
- Headers: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`.
- Model: `claude-opus-5-5` by default. `claude-sonnet-5-5` can be selected as the cheaper option.
- Thinking is adaptive; it's the default on Opus 5.5 and can't be turned off. Cost is controlled with `output_config.effort`, set per step:
  - ingest: `low`
  - analyze: `medium`
  - tailor: `high`
  - The tailor effort can be overridden in the config.
- Structured output uses `output_config: { format: { type: "json_schema", schema } }`, with the schema generated by `schemars`. The response is still validated with serde. No prefill.
- The server uses streaming (SSE) for its calls to Claude, to avoid HTTP timeouts on long outputs, with `max_tokens` around 32k. The frontend simply waits on the request and shows a spinner.
- Prompt caching: `cache_control` on the system prompt + master resume.

**Error handling**
- `stop_reason: refusal`: show it and offer a retry.
- `max_tokens`: retry once with a higher cap.
- Schema-validation failure: one automatic re-ask that includes the error.
- 429, 5xx and network errors: exponential backoff that honours `retry-after`.

**Logging:** each call writes step, model, tokens and cost to `llm_calls`.

**Rough cost on Opus 5.5:** about $0.05 to ingest (once), then about $0.10–0.25 per application. Sonnet 5.5 is roughly half.

**Prompts** live in `prompts/*.md` and are compiled in with `include_str!`. The system prompt sets four non-negotiables:
1. Never invent employers, titles, dates, metrics, tools or responsibilities.
2. Only rephrase content that's present in the referenced source item.
3. Report gaps in `notes` instead of filling them.
4. Follow the config rules exactly.

---

## 6. Storage

Everything lives in `./data/`, which is gitignored:
```
db.sqlite
resume.pdf               (original upload)
pdfs/{application_id}.pdf
```

Tables:
- `master_resume`: a single row with `json`, `source_pdf` and `updated_at`.
- `applications`:
  - identity: `id`, `company`, `title`
  - content: `jd_text`, `analysis_json`, `config_json`, `plan_json`, `decisions_json`, `final_json`, `pdf_path`
  - tracking: `status` (draft | exported | applied | interviewing | rejected | offer), `created_at`, `updated_at`
- `llm_calls`: `id`, `application_id`, `step`, `model`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cost_usd`, `created_at`

Schema migrations are plain SQL files run at startup.

---

## 7. HTTP API

```
GET    /api/health                       → { claude_connected }
POST   /api/resume/ingest   (multipart)  → Resume (unsaved draft)
GET    /api/resume                       → Resume | 404
PUT    /api/resume                       → save master resume

POST   /api/applications    { jd_text }  → Application (draft)
GET    /api/applications                 → list
GET    /api/applications/:id             → Application
PATCH  /api/applications/:id             { status | analysis | decisions | config }
POST   /api/applications/:id/analyze     → JobAnalysis
POST   /api/applications/:id/tailor      { config } → { plan, verify_report }
POST   /api/applications/:id/render      → { page_count, trimmed_ids[] }
GET    /api/applications/:id/pdf         → application/pdf
```

---

## 8. Frontend

Pages:
1. **Resume** (`/resume`): upload the PDF, then edit it in a structured form. Sections are reorderable, each item has a lock toggle, and there's a raw JSON tab.
2. **New application** (`/new`): a wizard that runs in order:
   1. Paste the JD.
   2. Review the analysis chips.
   3. Pick the config.
   4. Tailor.
   5. Review the diff.
   6. Preview and download.
3. **Applications** (`/`): a table with company, title, date and status. Each row opens the saved application and has a PDF download.
4. **Settings** (`/settings`): connection status, default model, effort and preset, and total spend.

---

## 9. PDF template

One plain, standard template written in Typst (`templates/standard.typ`):
- A name header with one line of contact info and links.
- Sections in this order by default: Summary, Experience, Projects, Skills, Education, Certifications. The order can be changed by the plan.
- A single column, left-aligned, with no photo, icons, tables, colour or graphics.
- A standard serif or sans font (e.g. Source Sans 3 or Libertinus), embedded in the PDF. Body text is 10–11pt.
- Each role has a bold title and company on one line, with the dates right-aligned, followed by bullets.
- All text is real and selectable, with clickable hyperlinks.

---

## 10. Project layout

```
resume-tailor/
  Cargo.toml              (workspace)
  crates/
    core/
    server/
    cli/
  web/                    (Vite + React + TS)
  prompts/
  templates/standard.typ
  fixtures/               (sample resume PDF, JDs, expected JSON)
  .env.example
```

Dev: `cargo run -p server` + `npm run dev` in `web/`. Prod: build `web/`, then `cargo run --release -p server`, which serves the built frontend.

---

## 11. Milestones

| # | Milestone | Done when |
|---|---|---|
| M0 | Scaffold | Workspace builds; server answers `/api/health`; Vite app loads through the proxy |
| M1 | Schema + render (no LLM) | A hand-written `resume.json` renders to PDF via the CLI; the text is selectable; page count is correct |
| M2 | Claude client + ingest | `cli ingest resume.pdf` → valid `Resume` JSON; calls are logged with cost |
| M3 | JD analysis | `cli analyze jd.txt` → sensible `JobAnalysis` on 3 sample JDs |
| M4 | Tailor + verify | `cli tailor` → plan → PDF end to end; verifier tests catch injected fake metrics and tools |
| M5 | Web: resume page | Upload, edit and save the master resume in the browser |
| M6 | Web: application wizard | Full flow in the browser: analyze, tailor, review, preview, download |
| M7 | Web: applications list + settings | History, status updates, spend total |

---

## 12. Testing

- **Unit:** verifier rules, applying a plan, schema round-trips.
- **Golden:** render fixtures, then check the extracted text and page count.
- **Contract:** recorded Claude responses as fixtures, so tests never call the API.
- **Manual eval:** about 5 JDs against your resume. Pass means zero hard verifier failures plus a skim for quality. Re-run it whenever a prompt changes.

---

## 13. Risks

| Risk | Mitigation |
|---|---|
| Invented experience | No-create schema + deterministic verifier + human review |
| Bad parse of a fancy PDF | Editable form after ingest |
| Page overflow | Relevance-based trimming that the user can see and undo |
| Embedding Typst is fiddly | Spike it in M1; fall back to shelling out to the `typst` CLI |
