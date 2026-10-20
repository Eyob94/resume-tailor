import { Sparkles, UploadCloud } from "lucide-react";
import { useResume } from "../api";
import { ButtonLink, EmptyState, PageHeader, Skeleton } from "../components/ui";

export function NewApplicationPage() {
  const { state } = useResume();

  return (
    <>
      <PageHeader title="New application" description="Tailor your master resume to a specific job description." />
      {state.status === "loading" ? (
        <Skeleton className="h-64" />
      ) : state.status === "ready" && state.data ? (
        <EmptyState icon={Sparkles} title="Tailoring is coming next">
          Your master resume is ready. Job description analysis and tailoring land once the AI backend is chosen.
        </EmptyState>
      ) : (
        <EmptyState
          icon={UploadCloud}
          title="Upload your master resume first"
          action={
            <ButtonLink to="/resume" variant="primary" icon={UploadCloud}>
              Upload master resume
            </ButtonLink>
          }
        >
          Tailoring needs a baseline to work from.
        </EmptyState>
      )}
    </>
  );
}
