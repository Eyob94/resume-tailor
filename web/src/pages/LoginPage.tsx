import { useState, type FormEvent } from "react";
import { api, errorMessage } from "../api";

export function LoginPage() {
  const [mode, setMode] = useState<"login" | "register">("login");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (mode === "register") await api.register(username, password);
      else await api.login(username, password);
      window.location.assign("/");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-zinc-50 px-4 py-10">
      <section className="w-full max-w-sm rounded-2xl border border-zinc-200 bg-white p-7 shadow-sm">
        <div className="mb-7 flex items-center gap-3">
          <img src="/favicon.svg" alt="" className="size-8" />
          <div>
            <h1 className="font-semibold tracking-tight">Resume Tailor</h1>
            <p className="text-sm text-zinc-500">{mode === "login" ? "Sign in to your account" : "Create a personal account"}</p>
          </div>
        </div>
        {error && <p role="alert" className="mb-4 rounded-lg bg-red-50 px-3 py-2 text-sm text-red-700">{error}</p>}
        <form onSubmit={submit} className="space-y-4">
          <label className="block text-sm font-medium">
            Username
            <input
              autoComplete="username"
              required
              minLength={3}
              maxLength={32}
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              className="mt-1.5 w-full rounded-lg border border-zinc-200 px-3 py-2 font-normal outline-none focus:border-zinc-500"
            />
          </label>
          <label className="block text-sm font-medium">
            Password
            <input
              type="password"
              autoComplete={mode === "login" ? "current-password" : "new-password"}
              required
              minLength={mode === "register" ? 12 : undefined}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              className="mt-1.5 w-full rounded-lg border border-zinc-200 px-3 py-2 font-normal outline-none focus:border-zinc-500"
            />
            {mode === "register" && <span className="mt-1 block text-xs font-normal text-zinc-500">Use at least 12 characters.</span>}
          </label>
          <button disabled={busy} className="w-full rounded-lg bg-zinc-900 px-3 py-2.5 text-sm font-medium text-white disabled:opacity-50">
            {busy ? "Please wait…" : mode === "login" ? "Sign in" : "Create account"}
          </button>
        </form>
        <p className="mt-5 text-center text-sm text-zinc-500">
          {mode === "login" ? "New here?" : "Already have an account?"}{" "}
          <button className="font-medium text-zinc-900 underline underline-offset-4" onClick={() => { setMode(mode === "login" ? "register" : "login"); setError(null); }}>
            {mode === "login" ? "Create an account" : "Sign in"}
          </button>
        </p>
      </section>
    </main>
  );
}
