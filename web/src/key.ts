const KEY_STORAGE = "rt.key";
const KEY_SAVED_STORAGE = "rt.keySaved";

export function getKey(): string | null {
  try {
    return localStorage.getItem(KEY_STORAGE);
  } catch {
    return null;
  }
}

export function setKey(key: string): void {
  try {
    localStorage.setItem(KEY_STORAGE, key);
  } catch {
  }
}

export function clearKey(): void {
  try {
    localStorage.removeItem(KEY_STORAGE);
  } catch {
  }
}

export function isKeySaved(): boolean {
  try {
    return localStorage.getItem(KEY_SAVED_STORAGE) === "1";
  } catch {
    return false;
  }
}

export function markKeySaved(): void {
  try {
    localStorage.setItem(KEY_SAVED_STORAGE, "1");
  } catch {
  }
}
