import { useState } from "react";
import { Check, Copy, Download } from "lucide-react";
import { Button } from "./ui";

export function JsonView({ value, fileName, note }: { value: unknown; fileName: string; note?: string }) {
  const [copied, setCopied] = useState(false);
  const json = JSON.stringify(value, null, 2);

  async function copy() {
    try {
      await navigator.clipboard.writeText(json);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard blocked (e.g. insecure context); Download still works.
    }
  }

  function download() {
    const url = URL.createObjectURL(new Blob([json], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = fileName;
    link.click();
    URL.revokeObjectURL(url);
  }

  return (
    <div>
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-zinc-200 px-4 py-3">
        <p className="text-xs text-zinc-500">{note}</p>
        <div className="flex gap-2">
          <Button icon={copied ? Check : Copy} onClick={copy}>
            {copied ? "Copied" : "Copy"}
          </Button>
          <Button icon={Download} onClick={download}>
            Download
          </Button>
        </div>
      </div>
      <pre className="max-h-[75vh] overflow-auto bg-zinc-950 break-words whitespace-pre-wrap p-4 font-mono text-xs leading-relaxed text-zinc-100">{json}</pre>
    </div>
  );
}
