import { Briefcase, Sparkles, UploadCloud } from "lucide-react";
import { useResume } from "../api";
import { ButtonLink, EmptyState, PageHeader, Skeleton } from "../components/ui";

export function ApplicationsPage() {
  const { state } = useResume();
  const hasResume = state.status === "ready" && state.data !== null;

  return (
    <>
      <PageHeader
        title="Applications"
        description="Every tailored resume you've created."
        actions={
          hasResume && (
            <ButtonLink to="/new" variant="primary" icon={Sparkles}>
              New application
            </ButtonLink>
          )
        }
      />
      {state.status === "loading" ? (
        <Skeleton className="h-64" />
      ) : (
        <EmptyState
          icon={Briefcase}
          title="No applications yet"
          action={
            hasResume ? (
              <ButtonLink to="/new" variant="primary" icon={Sparkles}>
                Tailor your first resume
              </ButtonLink>
            ) : (
              <ButtonLink to="/resume" variant="primary" icon={UploadCloud}>
                Upload master resume
              </ButtonLink>
            )
          }
        >
          {hasResume
            ? "Paste a job description to create a tailored resume for it."
            : "Start by uploading your master resume. It's the baseline every tailored version is built from."}
        </EmptyState>
      )}
    </>
  );
}
