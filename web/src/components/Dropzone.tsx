import { useRef, useState } from "react";
import { Loader2, UploadCloud } from "lucide-react";

export function Dropzone({ onFile, busy }: { onFile: (file: File) => void; busy: boolean }) {
  const [dragging, setDragging] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  const browse = () => {
    if (!busy) input.current?.click();
  };

  return (
    <div
      role="button"
      tabIndex={0}
      aria-busy={busy}
      onClick={browse}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          browse();
        }
      }}
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        const file = e.dataTransfer.files[0];
        if (file && !busy) onFile(file);
      }}
      className={`flex cursor-pointer flex-col items-center justify-center rounded-xl border-2 border-dashed px-6 py-20 text-center transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-zinc-900 ${
        dragging ? "border-zinc-900 bg-zinc-100" : "border-zinc-300 bg-white hover:border-zinc-400 hover:bg-zinc-50"
      } ${busy ? "cursor-wait" : ""}`}
    >
      <input
        ref={input}
        type="file"
        accept="application/pdf,.pdf"
        className="hidden"
        onChange={(e) => {
          const file = e.target.files?.[0];
          e.target.value = "";
          if (file) onFile(file);
        }}
      />
      <div className="pointer-events-none flex flex-col items-center">
        <div className="mb-4 flex size-12 items-center justify-center rounded-full bg-zinc-100">
          {busy ? <Loader2 className="size-6 animate-spin text-zinc-500" /> : <UploadCloud className="size-6 text-zinc-500" />}
        </div>
        <p className="text-base font-semibold">{busy ? "Uploading…" : "Drop your resume PDF here"}</p>
        <p className="mt-1 text-sm text-zinc-500">{busy ? "Reading your resume" : "or click to browse · PDF up to 10 MB"}</p>
      </div>
    </div>
  );
}
