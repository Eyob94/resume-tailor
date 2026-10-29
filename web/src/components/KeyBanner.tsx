import { useState } from "react";
import { Copy } from "lucide-react";
import { getKey, isKeySaved, markKeySaved } from "../key";
import { Button } from "./ui";

export function KeyBanner() {
  const [visible, setVisible] = useState(() => !isKeySaved() && getKey() !== null);

  if (!visible) return null;

  function copyKey() {
    const key = getKey();
    if (!key) return;
    navigator.clipboard.writeText(key).then(
      () => {
        markKeySaved();
        setVisible(false);
      },
      () => undefined,
    );
  }

  return (
    <div className="mb-6 flex flex-col gap-3 rounded-lg border border-zinc-200 bg-white px-4 py-3 text-sm shadow-sm sm:flex-row sm:items-center sm:justify-between">
      <p className="text-zinc-700">Save your key to open this account on another device.</p>
      <div className="flex gap-2">
        <Button variant="primary" icon={Copy} onClick={copyKey}>
          Copy key
        </Button>
        <Button onClick={() => setVisible(false)}>Later</Button>
      </div>
    </div>
  );
}
