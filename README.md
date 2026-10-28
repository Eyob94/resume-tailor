# Resume Tailor

Multi-user web app that tailors a master resume to job descriptions. Each account has its own resume, job searches, saved jobs, and settings. See [SPEC.md](SPEC.md).

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

Set `DATABASE_URL` to a direct PostgreSQL connection string before starting the server. Use a direct Neon connection (the host without `-pooler`) or a Supabase session pooler on port 5432. Transaction-mode poolers are not compatible with SQLx prepared statements.

Open http://localhost:8787 and create an account. The server creates its schema on an empty PostgreSQL database; existing SQLite data is not imported.

### Frontend dev (hot reload)

Run the server as above, then in another terminal:

```bash
cd web && npm run dev
```

Open http://localhost:5173 (proxies `/api` to the Rust server).

## VPS deployment

GitHub Actions runs the Rust tests against PostgreSQL and builds the frontend on pull requests. A push to `main` also builds a release and deploys it to an Ubuntu VPS over IPv4 SSH. Each deploy first runs `deploy/bootstrap.sh` on the server, which installs Caddy, the systemd service and the deploy script if they're missing; it's safe to re-run. Resume and application data lives in PostgreSQL, and Caddy serves `codpal.io` over HTTPS, proxying to the app on `127.0.0.1:8787`.

One-time setup:

1. Point the `codpal.io` A record at the VPS IPv4 address, add a `www` CNAME to `codpal.io` (it redirects to `codpal.io`), and allow inbound TCP ports 22, 80 and 443.
2. Add a public key for GitHub Actions to `~/.ssh/authorized_keys` of the deploy user. That user must be `root` or have passwordless sudo.
3. In GitHub, create the `production` environment with these secrets:

   - `VPS_HOST`: the VPS IPv4 address, or a hostname that resolves to it.
   - `VPS_USER`: the deploy user, e.g. `root`.
   - `VPS_SSH_PRIVATE_KEY`: the matching private key.
   - `DATABASE_URL`: a direct PostgreSQL connection string.

   The deploy skips SSH host-key verification, so it does not check the server's identity.

4. Optional: edit `/etc/resume-tailor.env` on the server after the first deploy, e.g. to point `OLLAMA_URL` at another machine. Later deploys keep your changes.

Each release is health-checked; a failed restart or check restores the previous release. User data stays outside the release folders.

## Config (env vars)

| Var | Default | |
|---|---|---|
| `PORT` | `8787` | Port for the local server, which binds to 127.0.0.1 |
| `DATABASE_URL` | required | Direct PostgreSQL connection string |
| `WEB_DIST` | `web/dist` | Built frontend to serve |
| `OLLAMA_URL` | `http://127.0.0.1:11434` | Initial Ollama server URL; can be changed later in Settings and is saved in PostgreSQL |
| `SCAN_COOLDOWN_SECONDS` | `600` | Minimum gap before a manual "Run now" scan. Lower only for local testing (the dev launch config uses 60) |

## Tests

```bash
cargo test --workspace
cd web && npm run typecheck
```

Rust tests require a disposable PostgreSQL database. For a local throwaway instance:

```bash
docker run -d --name tailor-pg -e POSTGRES_PASSWORD=postgres -p 5433:5432 postgres:16
DATABASE_URL=postgres://postgres:postgres@localhost:5433/postgres cargo test --workspace
docker rm -f tailor-pg
```
