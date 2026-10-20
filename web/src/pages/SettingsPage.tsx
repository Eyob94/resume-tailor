import type { ReactNode } from "react";
import { ServerStatus } from "../components/Layout";
import { Card, PageHeader } from "../components/ui";

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1 px-5 py-4 sm:flex-row sm:items-center sm:justify-between">
      <span className="text-sm font-medium">{label}</span>
      <span className="text-sm text-zinc-500">{children}</span>
    </div>
  );
}

export function SettingsPage() {
  return (
    <>
      <PageHeader title="Settings" />
      <Card className="divide-y divide-zinc-100">
        <Row label="Server">
          <ServerStatus />
        </Row>
        <Row label="AI backend">Not configured yet</Row>
        <Row label="Data">
          Stored locally in <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-xs">./data</code>
        </Row>
      </Card>
    </>
  );
}
