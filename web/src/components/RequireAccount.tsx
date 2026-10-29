import { useEffect, useState } from "react";
import { Outlet } from "react-router-dom";
import { Loader2 } from "lucide-react";
import { api, ApiError } from "../api";
import { clearKey, getKey, setKey } from "../key";
import { Button } from "./ui";

function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

async function signIn(): Promise<void> {
  try {
    await api.currentUser();
    return;
  } catch (error) {
    if (!isUnauthorized(error)) throw error;
  }
  const stored = getKey();
  if (stored) {
    try {
      await api.keyLogin(stored);
      return;
    } catch (error) {
      if (!isUnauthorized(error)) throw error;
      clearKey();
    }
  }
  const { key } = await api.createKey();
  setKey(key);
}

let signingIn: Promise<void> | null = null;

function ensureSignedIn(): Promise<void> {
  signingIn ??= signIn().finally(() => {
    signingIn = null;
  });
  return signingIn;
}

export function RequireAccount() {
  const [state, setState] = useState<"checking" | "offline" | "ready">("checking");

  function start() {
    setState("checking");
    ensureSignedIn().then(
      () => setState("ready"),
      () => setState("offline"),
    );
  }

  useEffect(start, []);

  if (state === "ready") return <Outlet />;
  if (state === "offline")
    return (
      <div className="flex min-h-screen flex-col items-center justify-center gap-4 text-sm text-zinc-500">
        <p>Can't reach the server.</p>
        <Button onClick={start}>Retry</Button>
      </div>
    );
  return (
    <div className="flex min-h-screen items-center justify-center" role="status" aria-label="Signing in">
      <Loader2 className="size-6 animate-spin text-zinc-400" />
    </div>
  );
}
