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

Open http://localhost:8787 and create an account. Existing data in `DATA_DIR` is assigned to the first account created; later accounts get separate data directories under `DATA_DIR/users/`.

### Frontend dev (hot reload)

Run the server as above, then in another terminal:

```bash
cd web && npm run dev
```

Open http://localhost:5173 (proxies `/api` to the Rust server).

## VPS deployment

GitHub Actions runs the Rust workspace tests and frontend production build on pull requests. A push to `main` also builds a release binary and deploys it to an Ubuntu 24.04 x86_64 VPS over IPv6 SSH. The app runs under systemd, with persistent data at `/var/lib/resume-tailor`; Caddy serves `ticklake.io` over HTTPS and proxies to the app on `127.0.0.1:8787`.

One-time VPS setup:

1. Point the `ticklake.io` AAAA record at the VPS IPv6 address and allow inbound TCP ports 22, 80, and 443.
2. Install Caddy on the VPS using its [official installation instructions](https://caddyserver.com/docs/install).
3. Create a non-root SSH deployment user and add a public key for GitHub Actions to that user's `~/.ssh/authorized_keys`.
4. From a checkout of this repository on the VPS, run `sudo ./deploy/bootstrap.sh <deploy-user>`.
5. Edit `/etc/resume-tailor.env`. Set `OLLAMA_URL` to the URL of the Ollama machine reachable from the VPS. Keep `PORT`, `DATA_DIR`, and `WEB_DIST` as provided.
6. In GitHub, create the `production` environment and add these environment secrets:

   - `VPS_HOST`: the VPS IPv6 address (without brackets), or a hostname that resolves to it.
   - `VPS_USER`: the non-root deployment username.
   - `VPS_SSH_PRIVATE_KEY`: the matching private key.
   - `VPS_KNOWN_HOSTS`: the VPS SSH host key line, gathered with `ssh-keyscan -6 <vps-ipv6>` and verified against the VPS.

After setup, pushes to `main` deploy automatically. Pull requests run checks without deployment. Each release is health-checked; a failed restart or check restores the previous release. User databases and uploaded resumes remain outside release directories. The VPS must be reachable by the GitHub-hosted runner over IPv6 SSH.

## Config (env vars)

| Var | Default | |
|---|---|---|
| `PORT` | `8787` | Port for the local server, which binds to 127.0.0.1 |
| `DATA_DIR` | `data` | SQLite DB + uploaded PDF |
| `WEB_DIST` | `web/dist` | Built frontend to serve |
| `OLLAMA_URL` | `http://127.0.0.1:11434` | Initial Ollama server URL; can be changed later in Settings and is saved in SQLite |
| `SCAN_COOLDOWN_SECONDS` | `600` | Minimum gap before a manual "Run now" scan. Lower only for local testing (the dev launch config uses 60) |

## Tests

```bash
cargo test --workspace
cd web && npm run typecheck
```
