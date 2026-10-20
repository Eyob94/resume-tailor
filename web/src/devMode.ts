import { useCallback, useEffect, useState } from "react";

const STORAGE_KEY = "resume-tailor:dev-mode";
const CHANGE_EVENT = "dev-mode-change";

function read(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

/** Per-browser developer mode flag, kept in sync across components and tabs. */
export function useDevMode(): [boolean, (on: boolean) => void] {
  const [enabled, setEnabled] = useState(read);

  useEffect(() => {
    const sync = () => setEnabled(read());
    window.addEventListener(CHANGE_EVENT, sync);
    window.addEventListener("storage", sync);
    return () => {
      window.removeEventListener(CHANGE_EVENT, sync);
      window.removeEventListener("storage", sync);
    };
  }, []);

  const set = useCallback((on: boolean) => {
    try {
      if (on) localStorage.setItem(STORAGE_KEY, "1");
      else localStorage.removeItem(STORAGE_KEY);
    } catch {
      // Storage unavailable (private mode etc.); the flag just won't persist.
    }
    setEnabled(on);
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }, []);

  return [enabled, set];
}
