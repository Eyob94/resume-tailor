# Resume Tailor

Personal web app that tailors a master resume to job descriptions. See [SPEC.md](SPEC.md).

**Status:** app shell + master resume upload (store PDF, preview, extracted text). AI backend not chosen yet.

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

## Tests

```bash
cargo test --workspace
cd web && npm run typecheck
```
