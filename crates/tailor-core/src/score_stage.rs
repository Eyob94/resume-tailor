use std::{collections::HashSet, sync::Arc};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
};

use crate::{
    db::Db,
    errors::{AiError, ScoreError},
    parse_stage::{configured_ollama_url, JsonModel, Ollama, DEFAULT_MODEL, MODEL_KEY},
    types::{Importance, JobStatus, JobToScore, ProfileRecord, Requirement, ResumeProfile, Verdict},
};

pub enum ScoreMessage {
    Kick,
    Rescore {
        job_id: i64,
        reply: oneshot::Sender<Result<(), ScoreError>>,
    },
    Skip {
        job_id: i64,
        reply: oneshot::Sender<Result<(), ScoreError>>,
    },
}

pub struct ScoreStage {
    handle: JoinHandle<()>,
}

impl ScoreStage {
    pub fn new(db: Arc<Db>, default_ollama_url: String, rx: UnboundedReceiver<ScoreMessage>) -> Self {
        let handle = tokio::spawn(run(db, default_ollama_url, rx));
        Self { handle }
    }

    pub async fn join(self) {
        let _ = self.handle.await;
    }
}

async fn run(db: Arc<Db>, default_ollama_url: String, mut rx: UnboundedReceiver<ScoreMessage>) {
    if let Err(e) = db.requeue_interrupted_scores() {
        tracing::error!("couldn't requeue interrupted job scores: {e}");
    }
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut running = false;
    let mut wanted = true;
    loop {
        if wanted && !running {
            wanted = false;
            match next_job(&db) {
                Ok(Some((profile_id, profile, job))) => {
                    running = true;
                    let url = configured_ollama_url(&db, &default_ollama_url);
                    spawn_scoring(db.clone(), url, profile_id, profile, job, done_tx.clone());
                }
                Ok(None) => {}
                Err(e) => tracing::error!("couldn't pick the next job to score: {e}"),
            }
        }
        tokio::select! {
            message = rx.recv() => match message {
                None => break,
                Some(ScoreMessage::Kick) => wanted = true,
                Some(ScoreMessage::Rescore { job_id, reply }) => {
                    let result = rescore(&db, job_id);
                    if result.is_ok() {
                        wanted = true;
                    }
                    let _ = reply.send(result);
                }
                Some(ScoreMessage::Skip { job_id, reply }) => {
                    let _ = reply.send(skip(&db, job_id));
                }
            },
            _ = done_rx.recv(), if running => {
                running = false;
                wanted = true;
            }
        }
    }
}

fn next_job(db: &Db) -> Result<Option<(i64, ResumeProfile, JobToScore)>, ScoreError> {
    let Some(ProfileRecord { id, profile: Some(profile), .. }) = db.current_confirmed_profile()? else {
        return Ok(None);
    };
    Ok(db.next_job_to_score(id)?.map(|job| (id, profile, job)))
}

fn rescore(db: &Db, job_id: i64) -> Result<(), ScoreError> {
    match db.job_status(job_id)? {
        None => return Err(ScoreError::NotFound),
        Some(JobStatus::Saved) => {}
        Some(_) => return Err(ScoreError::NotSaved),
    }
    if db.current_confirmed_profile()?.is_none() {
        return Err(ScoreError::NoProfile);
    }
    if db.score_status(job_id)?.as_deref() == Some("scoring") {
        return Err(ScoreError::AlreadyScoring);
    }
    db.clear_score(job_id)?;
    Ok(())
}

fn skip(db: &Db, job_id: i64) -> Result<(), ScoreError> {
    match db.job_status(job_id)? {
        None => return Err(ScoreError::NotFound),
        Some(JobStatus::Saved) => {}
        Some(_) => return Err(ScoreError::NotSaved),
    }
    let profile_id = db.current_confirmed_profile()?.map(|p| p.id).unwrap_or(0);
    db.skip_score(job_id, profile_id)?;
    Ok(())
}

fn spawn_scoring(
    db: Arc<Db>,
    ollama_url: String,
    profile_id: i64,
    profile: ResumeProfile,
    job: JobToScore,
    done: UnboundedSender<()>,
) {
    tokio::spawn(async move {
        let model_name = db
            .get_setting(MODEL_KEY)
            .ok()
            .flatten()
            .unwrap_or_else(|| DEFAULT_MODEL.into());
        let model = Ollama::new(&ollama_url, &model_name);
        let outcome = match db.start_score(job.job_id, profile_id, &model.label()) {
            Ok(()) => {
                tracing::info!(job_id = job.job_id, model = %model.label(), "scoring job");
                match score_job(&model, &profile, &job).await {
                    Ok(scored) => db.finish_score(job.job_id, scored.score, &scored.summary, &scored.requirements),
                    Err(e) => {
                        tracing::warn!(job_id = job.job_id, "scoring failed: {e}");
                        db.fail_score(job.job_id, &e.to_string())
                    }
                }
            }
            Err(e) => Err(e),
        };
        if let Err(e) = outcome {
            tracing::error!(job_id = job.job_id, "couldn't save job score: {e}");
        }
        let _ = done.send(());
    });
}

const SYSTEM_PROMPT: &str = "You check how well a candidate's resume fits a job. \
Step 1: read only the JOB DESCRIPTION and list every requirement it asks for: skills, technologies, years of experience, education, languages and other qualifications. \
Include every item from its requirements, qualifications and nice-to-have lists. \
requirement: a short phrase taken from the job description, for example \"Kafka or other event streaming\". Never put resume text in requirement. \
importance: \"must\" if the job says it is required, essential or a minimum, \"nice\" if it is preferred, a plus or optional. When unclear, use \"must\". \
Step 2: for each requirement, look in the RESUME and decide \"met\", \"partial\" or \"missing\". \
\"partial\" means related but weaker, for example a similar technology or fewer years than asked. \
If nothing in the resume mentions it, it is \"missing\". Do not assume anything the resume does not say. \
evidence: for met and partial, the exact words from the resume that show it; for missing, null. \
summary: two short sentences to the candidate, written as \"you\", on how well they fit.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "requirements": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "requirement": { "type": "string" },
                        "importance": { "type": "string", "enum": ["must", "nice"] },
                        "verdict": { "type": "string", "enum": ["met", "partial", "missing"] },
                        "evidence": { "type": ["string", "null"] },
                    },
                    "required": ["requirement", "importance", "verdict", "evidence"],
                },
            },
        },
        "required": ["summary", "requirements"],
    })
}

#[derive(Deserialize)]
struct RawScore {
    summary: String,
    requirements: Vec<RawRequirement>,
}

#[derive(Deserialize)]
struct RawRequirement {
    requirement: String,
    importance: Importance,
    verdict: Verdict,
    evidence: Option<String>,
}

impl From<RawRequirement> for Requirement {
    fn from(raw: RawRequirement) -> Self {
        Self { text: raw.requirement, importance: raw.importance, verdict: raw.verdict, evidence: raw.evidence }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoredJob {
    pub score: u8,
    pub summary: String,
    pub requirements: Vec<Requirement>,
}

pub fn resume_text(profile: &ResumeProfile) -> String {
    let mut lines = vec![profile.contact.name.clone()];
    lines.extend(profile.headline.clone());
    lines.extend(profile.summary.clone());
    for job in &profile.experience {
        let dates = [job.start.as_deref(), job.end.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" - ");
        lines.push(format!("{} at {} ({dates})", job.title, job.company));
        lines.extend(job.bullets.iter().map(|b| format!("- {}", b.text)));
    }
    for project in &profile.projects {
        lines.push(format!("Project: {}", project.name));
        lines.extend(project.description.clone());
        lines.extend(project.bullets.iter().map(|b| format!("- {}", b.text)));
    }
    for school in &profile.education {
        let degree = [school.degree.as_deref(), school.field.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(", ");
        lines.push(format!("Education: {degree} at {}", school.school));
    }
    if !profile.skills.is_empty() {
        let skills = profile
            .skills
            .iter()
            .map(|s| match s.years {
                Some(years) => format!("{} ({years} years)", s.name),
                None => s.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("Skills: {skills}"));
    }
    for (label, list) in [
        ("Certifications", &profile.certifications),
        ("Languages", &profile.languages),
        ("Awards", &profile.awards),
    ] {
        if !list.is_empty() {
            lines.push(format!("{label}: {}", list.join(", ")));
        }
    }
    lines.retain(|line| !line.trim().is_empty());
    lines.join("\n")
}

fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '+' && c != '#')
        .filter(|w| w.chars().count() > 2)
        .map(str::to_lowercase)
        .collect()
}

pub fn grounded_in(requirement: &str, job_description: &str) -> bool {
    let wanted = words(requirement);
    if wanted.is_empty() {
        return true;
    }
    let available: HashSet<String> = words(job_description).into_iter().collect();
    let found = wanted.iter().filter(|w| available.contains(*w)).count();
    found * 2 >= wanted.len()
}

pub fn tidy_requirements(raw: Vec<Requirement>, resume: &str, job_description: &str) -> Vec<Requirement> {
    let resume = normalize(resume);
    let mut seen = HashSet::new();
    raw.into_iter()
        .filter_map(|mut req| {
            req.text = req.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if req.text.is_empty() || !seen.insert(req.text.to_lowercase()) || !grounded_in(&req.text, job_description) {
                return None;
            }
            req.evidence = req
                .evidence
                .map(|e| e.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|e| {
                    let e = normalize(e);
                    !e.is_empty() && resume.contains(&e)
                });
            if req.verdict == Verdict::Missing {
                req.evidence = None;
            } else if req.verdict == Verdict::Met && req.evidence.is_none() {
                req.verdict = Verdict::Partial;
            }
            Some(req)
        })
        .collect()
}

pub fn score(requirements: &[Requirement]) -> u8 {
    let weight = |r: &Requirement| match r.importance {
        Importance::Must => 2.0,
        Importance::Nice => 1.0,
    };
    let credit = |r: &Requirement| match r.verdict {
        Verdict::Met => 1.0,
        Verdict::Partial => 0.5,
        Verdict::Missing => 0.0,
    };
    let total: f64 = requirements.iter().map(weight).sum();
    if total == 0.0 {
        return 0;
    }
    let earned: f64 = requirements.iter().map(|r| weight(r) * credit(r)).sum();
    (earned / total * 100.0).round() as u8
}

pub async fn score_job<M: JsonModel>(model: &M, profile: &ResumeProfile, job: &JobToScore) -> Result<ScoredJob, AiError> {
    let resume = resume_text(profile);
    let user = format!(
        "Job: {} at {}\n\nJob description:\n<<<\n{}\n>>>\n\nResume:\n<<<\n{resume}\n>>>",
        job.title, job.company, job.description
    );
    let schema = schema();
    let mut last_error = None;
    for _ in 0..2 {
        let value = match model.generate_json(SYSTEM_PROMPT, &user, &schema).await {
            Ok(value) => value,
            Err(AiError::BadOutput(e)) => {
                last_error = Some(AiError::BadOutput(e));
                continue;
            }
            Err(e) => return Err(e),
        };
        match serde_json::from_value::<RawScore>(value) {
            Ok(raw) => {
                let raw_requirements = raw.requirements.into_iter().map(Requirement::from).collect();
                let requirements = tidy_requirements(raw_requirements, &resume, &job.description);
                if requirements.is_empty() {
                    last_error = Some(AiError::BadOutput("no requirements found".into()));
                    continue;
                }
                return Ok(ScoredJob {
                    score: score(&requirements),
                    summary: raw.summary.split_whitespace().collect::<Vec<_>>().join(" "),
                    requirements,
                });
            }
            Err(e) => last_error = Some(AiError::BadOutput(e.to_string())),
        }
    }
    Err(last_error.expect("loop ran"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::types::{Bullet, Contact, Experience, Skill};

    struct FakeModel {
        answers: Mutex<Vec<Result<Value, AiError>>>,
    }

    impl JsonModel for FakeModel {
        fn label(&self) -> String {
            "fake".into()
        }

        async fn generate_json(&self, _: &str, _: &str, _: &Value) -> Result<Value, AiError> {
            self.answers.lock().unwrap().remove(0)
        }
    }

    fn profile() -> ResumeProfile {
        ResumeProfile {
            contact: Contact { name: "Jordan Rivera".into(), ..Default::default() },
            experience: vec![Experience {
                id: "e1".into(),
                company: "Acme".into(),
                title: "Senior Engineer".into(),
                start: Some("2021".into()),
                end: Some("Present".into()),
                bullets: vec![Bullet { id: "e1.b1".into(), text: "Led migration of billing services to Rust".into() }],
                ..Default::default()
            }],
            skills: vec![Skill { name: "Rust".into(), years: Some(5.0), used_at: vec![] }],
            ..Default::default()
        }
    }

    fn job() -> JobToScore {
        JobToScore { job_id: 1, title: "Rust Engineer".into(), company: "Globex".into(), description: "Rust, Kafka".into() }
    }

    #[test]
    fn requirements_must_come_from_the_job_description() {
        let description = "5+ years of backend experience. Kafka or other event streaming platforms. C++ and C#.";
        assert!(grounded_in("Kafka or event streaming", description));
        assert!(grounded_in("C++", description));
        assert!(!grounded_in("Led migration of billing services to Rust", description));
    }

    fn req(text: &str, importance: Importance, verdict: Verdict, evidence: Option<&str>) -> Requirement {
        Requirement { text: text.into(), importance, verdict, evidence: evidence.map(str::to_string) }
    }

    #[test]
    fn score_weights_must_haves_double() {
        let reqs = vec![
            req("Rust", Importance::Must, Verdict::Met, None),
            req("Kafka", Importance::Must, Verdict::Missing, None),
            req("AWS", Importance::Nice, Verdict::Partial, None),
        ];
        assert_eq!(score(&reqs), 50);
        assert_eq!(score(&[]), 0);
    }

    #[test]
    fn tidy_drops_unfounded_evidence_and_downgrades_met() {
        let resume = resume_text(&profile());
        let description = "Rust, Kubernetes, Kafka and rust";
        let tidied = tidy_requirements(
            vec![
                req("Rust", Importance::Must, Verdict::Met, Some("Led migration of billing services\nto Rust")),
                req("Kubernetes", Importance::Must, Verdict::Met, Some("Ran Kubernetes clusters")),
                req("rust", Importance::Nice, Verdict::Met, None),
                req("Kafka", Importance::Must, Verdict::Missing, Some("Rust")),
                req("  ", Importance::Must, Verdict::Met, None),
                req("Led migration of billing services", Importance::Must, Verdict::Met, None),
            ],
            &resume,
            description,
        );
        assert_eq!(
            tidied,
            vec![
                req("Rust", Importance::Must, Verdict::Met, Some("Led migration of billing services to Rust")),
                req("Kubernetes", Importance::Must, Verdict::Partial, None),
                req("Kafka", Importance::Must, Verdict::Missing, None),
            ]
        );
    }

    #[test]
    fn resume_text_includes_jobs_bullets_and_skills() {
        let text = resume_text(&profile());
        assert!(text.contains("Senior Engineer at Acme (2021 - Present)"));
        assert!(text.contains("- Led migration of billing services to Rust"));
        assert!(text.contains("Skills: Rust (5 years)"));
    }

    #[tokio::test]
    async fn scores_a_job_and_retries_bad_output_once() {
        let answer = json!({
            "summary": "Strong Rust background.  Missing Kafka.",
            "requirements": [
                { "requirement": "Rust", "importance": "must", "verdict": "met", "evidence": "billing services to Rust" },
                { "requirement": "Kafka", "importance": "must", "verdict": "missing", "evidence": null }
            ]
        });
        let model = FakeModel { answers: Mutex::new(vec![Err(AiError::BadOutput("cut off".into())), Ok(answer)]) };
        let scored = score_job(&model, &profile(), &job()).await.unwrap();
        assert_eq!(scored.score, 50);
        assert_eq!(scored.summary, "Strong Rust background. Missing Kafka.");
        assert_eq!(scored.requirements.len(), 2);
    }

    #[tokio::test]
    async fn empty_requirements_fail() {
        let empty = json!({ "summary": "x", "requirements": [] });
        let model = FakeModel { answers: Mutex::new(vec![Ok(empty.clone()), Ok(empty)]) };
        assert!(matches!(score_job(&model, &profile(), &job()).await, Err(AiError::BadOutput(_))));
    }
}
