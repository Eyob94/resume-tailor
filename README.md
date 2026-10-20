# Resume Tailor

Personal web app that tailors a master resume to job descriptions. See [SPEC.md](SPEC.md).

**Status:** app shell, master resume upload, and a Jobs tab that scans LinkedIn's signed-out job search for saved searches on a schedule (every 10–60 min). AI backend not chosen yet.

### LinkedIn scanning notes

- Uses only signed-out pages: the full search page (~60 jobs per title) and public job pages (`/jobs/view/{id}`) for descriptions. The `jobs-guest` API endpoints allow only a few requests per window, so they're avoided.
- 3–8 s random pause before every request; at most 10 descriptions per scan, and a refused job page pauses description fetching until the next scan.
- Work mode isn't exposed signed-out, so it's inferred from the text; unclear postings show as "not specified".

## Run

Requires Rust and Node 20+.

```bash
cd web && npm install && npm run build && cd ..
cargo run -p tailor-server
```

Open http://localhost:8787.

### Frontend dev (hot reload)

Run the server as above, then in another terminal:

```bash
cd web && npm run dev
```

Open http://localhost:5173 (proxies `/api` to the Rust server).

## Config (env vars)

| Var | Default | |
|---|---|---|
| `PORT` | `8787` | Server binds to 127.0.0.1 only (no auth) |
| `DATA_DIR` | `data` | SQLite DB + uploaded PDF |
| `WEB_DIST` | `web/dist` | Built frontend to serve |
| `SCAN_COOLDOWN_SECONDS` | `600` | Minimum gap before a manual "Run now" scan. Lower only for local testing (the dev launch config uses 60) |

## Tests

```bash
cargo test --workspace
cd web && npm run typecheck
```
