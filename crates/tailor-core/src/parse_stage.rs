use std::{collections::HashSet, future::Future, sync::Arc, time::Duration};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{
    sync::{mpsc::UnboundedReceiver, oneshot},
    task::JoinHandle,
};

use crate::{
    db::Db,
    errors::{AiError, DbError, ProfileError},
    types::{
        Bullet, Contact, Education, Experience, MasterResume, OllamaStatus, ProfileRecord, Project,
        ResumeProfile, Skill, Warning,
    },
};

pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
pub const MODEL_KEY: &str = "resume_reader_model";
pub const OLLAMA_URL_KEY: &str = "ollama_url";
pub const DEFAULT_MODEL: &str = "qwen2.5:7b";

pub enum ParseMessage {
    Parse {
        resume: MasterResume,
    },
    ReadAgain {
        reply: oneshot::Sender<Result<(), ProfileError>>,
    },
    SaveEdits {
        profile: ResumeProfile,
        reply: oneshot::Sender<Result<ProfileRecord, ProfileError>>,
    },
    Confirm {
        reply: oneshot::Sender<Result<ProfileRecord, ProfileError>>,
    },
}

pub struct ParseStage {
    handle: JoinHandle<()>,
}

impl ParseStage {
    pub fn new(db: Arc<Db>, ollama_url: String, rx: UnboundedReceiver<ParseMessage>) -> Self {
        let handle = tokio::spawn(run(db, ollama_url, rx));
        Self { handle }
    }

    pub async fn join(self) {
        let _ = self.handle.await;
    }
}

async fn run(db: Arc<Db>, default_ollama_url: String, mut rx: UnboundedReceiver<ParseMessage>) {
    let db_for_cleanup = db.clone();
    match tokio::task::spawn_blocking(move || db_for_cleanup.mark_interrupted_profiles()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!("couldn't tidy interrupted resume readings: {e}"),
        Err(e) => tracing::error!("couldn't tidy interrupted resume readings: {e}"),
    }
    while let Some(message) = rx.recv().await {
        match message {
            ParseMessage::Parse { resume } => {
                if let Err(e) = start_reading(
                    db.clone(),
                    configured_ollama_url(&db, &default_ollama_url),
                    resume,
                )
                .await
                {
                    tracing::error!("couldn't start reading the resume: {e}");
                }
            }
            ParseMessage::ReadAgain { reply } => {
                let db_for_read = db.clone();
                let result = tokio::task::spawn_blocking(move || db_for_read.get_resume())
                    .await
                    .map_err(|e| {
                        ProfileError::Db(DbError::Io(std::io::Error::other(e.to_string())))
                    })
                    .and_then(|result| result.map_err(ProfileError::Db))
                    .and_then(|resume| resume.ok_or(ProfileError::NoResume));
                let result = match result {
                    Ok(resume) => {
                        start_reading(
                            db.clone(),
                            configured_ollama_url(&db, &default_ollama_url),
                            resume,
                        )
                        .await
                    }
                    Err(e) => Err(e),
                };
                let _ = reply.send(result);
            }
            ParseMessage::SaveEdits { profile, reply } => {
                let result = save_edits(db.clone(), profile).await;
                let _ = reply.send(result);
            }
            ParseMessage::Confirm { reply } => {
                let result = confirm(db.clone()).await;
                let _ = reply.send(result);
            }
        }
    }
}

fn configured_ollama_url(db: &Db, default_url: &str) -> String {
    db.get_setting(OLLAMA_URL_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| default_url.to_string())
}

async fn start_reading(
    db: Arc<Db>,
    ollama_url: String,
    resume: MasterResume,
) -> Result<(), ProfileError> {
    let uploaded_at = resume.uploaded_at.clone();
    let db_for_start = db.clone();
    let url_for_start = ollama_url.clone();
    let (id, label, model_name) = tokio::task::spawn_blocking(move || {
        if let Some(latest) = db_for_start.latest_profile(&uploaded_at)? {
            if latest.status == "reading" {
                return Err(ProfileError::AlreadyReading);
            }
        }
        let model_name = db_for_start
            .get_setting(MODEL_KEY)?
            .unwrap_or_else(|| DEFAULT_MODEL.into());
        let model = Ollama::new(&url_for_start, &model_name);
        let label = JsonModel::label(&model);
        let id = db_for_start.start_profile(&uploaded_at, &label)?;
        Ok((id, label, model_name))
    })
    .await
    .map_err(|e| ProfileError::Db(DbError::Io(std::io::Error::other(e.to_string()))))??;
    let model = Ollama::new(&ollama_url, &model_name);
    tokio::spawn(async move {
        tracing::info!(model = %label, "reading resume");
        let started = std::time::Instant::now();
        let outcome = match read_resume(&model, &resume.extracted_text).await {
            Ok((profile, warnings)) => {
                let db = db.clone();
                tokio::task::spawn_blocking(move || db.finish_profile(id, &profile, &warnings))
                    .await
                    .unwrap_or_else(|e| Err(DbError::Io(std::io::Error::other(e.to_string()))))
            }
            Err(e) => {
                tracing::warn!("resume reading failed: {e}");
                let db = db.clone();
                tokio::task::spawn_blocking(move || db.fail_profile(id, &e.to_string()))
                    .await
                    .unwrap_or_else(|e| Err(DbError::Io(std::io::Error::other(e.to_string()))))
            }
        };
        if let Err(e) = outcome {
            tracing::error!("couldn't save resume reading: {e}");
        }
        tracing::info!(
            seconds = started.elapsed().as_secs(),
            "resume reading finished"
        );
    });
    Ok(())
}

async fn current_profile(db: Arc<Db>) -> Result<(MasterResume, ProfileRecord), ProfileError> {
    tokio::task::spawn_blocking(move || {
        let resume = db.get_resume()?.ok_or(ProfileError::NoResume)?;
        let profile = db
            .latest_profile(&resume.uploaded_at)?
            .ok_or(ProfileError::NotRead)?;
        Ok((resume, profile))
    })
    .await
    .map_err(|e| ProfileError::Db(DbError::Io(std::io::Error::other(e.to_string()))))?
}

async fn save_edits(
    db: Arc<Db>,
    mut profile: ResumeProfile,
) -> Result<ProfileRecord, ProfileError> {
    let (resume, record) = current_profile(db.clone()).await?;
    tidy(&mut profile);
    assign_ids(&mut profile);
    if profile.contact.name.is_empty() {
        return Err(ProfileError::MissingName);
    }
    let warnings = check_against(&profile, &resume.extracted_text);
    tokio::task::spawn_blocking(move || {
        if !db.save_profile_edits(record.id, &profile, &warnings)? {
            return Err(ProfileError::NotEditable);
        }
        db.get_profile(record.id)?.ok_or(ProfileError::NotRead)
    })
    .await
    .map_err(|e| ProfileError::Db(DbError::Io(std::io::Error::other(e.to_string()))))?
}

async fn confirm(db: Arc<Db>) -> Result<ProfileRecord, ProfileError> {
    let (_, record) = current_profile(db.clone()).await?;
    tokio::task::spawn_blocking(move || {
        if !db.confirm_profile(record.id)? {
            return Err(ProfileError::NotConfirmable);
        }
        db.get_profile(record.id)?.ok_or(ProfileError::NotRead)
    })
    .await
    .map_err(|e| ProfileError::Db(DbError::Io(std::io::Error::other(e.to_string()))))?
}

pub async fn ollama_status(base_url: &str) -> OllamaStatus {
    let url = base_url.trim_end_matches('/').to_string();
    let models = async {
        let res = reqwest::Client::new()
            .get(format!("{url}/api/tags"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .ok()?;
        let body: Value = res.json().await.ok()?;
        let mut names: Vec<String> = body["models"]
            .as_array()?
            .iter()
            .filter_map(|m| m["name"].as_str().map(str::to_string))
            .collect();
        names.sort();
        Some(names)
    }
    .await;
    OllamaStatus {
        url,
        reachable: models.is_some(),
        models: models.unwrap_or_default(),
    }
}

pub trait JsonModel {
    fn label(&self) -> String;

    fn generate_json(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> impl Future<Output = Result<Value, AiError>> + Send;
}

pub struct Ollama {
    http: reqwest::Client,
    base_url: String,
    model: String,
}

impl Ollama {
    pub fn new(base_url: &str, model: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .expect("static client config is valid");
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
        }
    }

    fn map_send_error(&self, e: reqwest::Error) -> AiError {
        if e.is_timeout() {
            AiError::Timeout
        } else {
            AiError::Unreachable(self.base_url.clone())
        }
    }
}

impl JsonModel for Ollama {
    fn label(&self) -> String {
        format!("ollama · {}", self.model)
    }

    async fn generate_json(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> Result<Value, AiError> {
        let body = json!({
            "model": self.model,
            "stream": false,
            "format": schema,
            "options": {
                "temperature": 0,
                "num_ctx": 16384,
            },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        let res = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| self.map_send_error(e))?;

        let status = res.status().as_u16();
        let text = res.text().await.map_err(|e| self.map_send_error(e))?;
        if status == 404 && text.contains("not found") {
            return Err(AiError::ModelMissing(self.model.clone()));
        }
        if !(200..300).contains(&status) {
            return Err(AiError::Status(status, text.chars().take(300).collect()));
        }

        let reply: Value =
            serde_json::from_str(&text).map_err(|e| AiError::BadOutput(e.to_string()))?;
        let content = reply["message"]["content"]
            .as_str()
            .ok_or_else(|| AiError::BadOutput("no message content".into()))?;
        serde_json::from_str(content).map_err(|e| AiError::BadOutput(e.to_string()))
    }
}

const SYSTEM_PROMPT: &str = "You extract structured data from a resume. \
Fill the JSON structure using only information found in the resume text. \
Copy names, job titles, company names and bullet points exactly as written: do not reword, shorten, merge or invent anything. \
Each bullet point is one list item; do not include the bullet symbol. \
Copy dates as written (for example \"Mar 2021\" or \"Present\"). \
Use null or an empty list when something is missing. \
For skills, list every skill, tool and technology mentioned, with years of experience when it can be worked out from the dates of the jobs that use it, and the companies or projects where it was used. \
headline: one short sentence describing the candidate, using only facts from the resume.";

fn nullable(kind: &str) -> Value {
    json!({ "type": [kind, "null"] })
}

fn strings() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "contact": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "email": nullable("string"),
                    "phone": nullable("string"),
                    "location": nullable("string"),
                    "links": strings(),
                },
                "required": ["name", "email", "phone", "location", "links"],
            },
            "summary": nullable("string"),
            "headline": nullable("string"),
            "experience": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "company": { "type": "string" },
                        "title": { "type": "string" },
                        "location": nullable("string"),
                        "start": nullable("string"),
                        "end": nullable("string"),
                        "bullets": strings(),
                    },
                    "required": ["company", "title", "location", "start", "end", "bullets"],
                },
            },
            "projects": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": nullable("string"),
                        "bullets": strings(),
                    },
                    "required": ["name", "description", "bullets"],
                },
            },
            "education": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "school": { "type": "string" },
                        "degree": nullable("string"),
                        "field": nullable("string"),
                        "start": nullable("string"),
                        "end": nullable("string"),
                    },
                    "required": ["school", "degree", "field", "start", "end"],
                },
            },
            "skills": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "years": nullable("number"),
                        "used_at": strings(),
                    },
                    "required": ["name", "years", "used_at"],
                },
            },
            "certifications": strings(),
            "languages": strings(),
            "awards": strings(),
        },
        "required": [
            "contact", "summary", "headline", "experience", "projects", "education",
            "skills", "certifications", "languages", "awards"
        ],
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawResume {
    contact: Contact,
    summary: Option<String>,
    headline: Option<String>,
    experience: Vec<RawExperience>,
    projects: Vec<RawProject>,
    education: Vec<Education>,
    skills: Vec<Skill>,
    certifications: Vec<String>,
    languages: Vec<String>,
    awards: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawExperience {
    company: String,
    title: String,
    location: Option<String>,
    start: Option<String>,
    end: Option<String>,
    bullets: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawProject {
    name: String,
    description: Option<String>,
    bullets: Vec<String>,
}

fn bullets(raw: Vec<String>) -> Vec<Bullet> {
    raw.into_iter()
        .map(|text| Bullet {
            id: String::new(),
            text,
        })
        .collect()
}

impl From<RawResume> for ResumeProfile {
    fn from(raw: RawResume) -> Self {
        Self {
            contact: raw.contact,
            summary: raw.summary,
            headline: raw.headline,
            experience: raw
                .experience
                .into_iter()
                .map(|e| Experience {
                    id: String::new(),
                    company: e.company,
                    title: e.title,
                    location: e.location,
                    start: e.start,
                    end: e.end,
                    bullets: bullets(e.bullets),
                })
                .collect(),
            projects: raw
                .projects
                .into_iter()
                .map(|p| Project {
                    id: String::new(),
                    name: p.name,
                    description: p.description,
                    bullets: bullets(p.bullets),
                })
                .collect(),
            education: raw.education,
            skills: raw.skills,
            certifications: raw.certifications,
            languages: raw.languages,
            awards: raw.awards,
        }
    }
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_opt(s: &mut Option<String>) {
    *s = s.as_deref().map(clean).filter(|v| !v.is_empty());
}

fn clean_list(list: &mut Vec<String>) {
    let mut seen = HashSet::new();
    list.retain_mut(|item| {
        *item = clean(item);
        !item.is_empty() && seen.insert(item.to_lowercase())
    });
}

fn clean_bullets(list: &mut Vec<Bullet>) {
    list.retain_mut(|b| {
        b.text = clean(b.text.trim_start_matches(['•', '-', '*', '–', '·']));
        !b.text.is_empty()
    });
}

pub fn tidy(profile: &mut ResumeProfile) {
    let c = &mut profile.contact;
    c.name = clean(&c.name);
    clean_opt(&mut c.email);
    clean_opt(&mut c.phone);
    clean_opt(&mut c.location);
    clean_list(&mut c.links);
    clean_opt(&mut profile.summary);
    clean_opt(&mut profile.headline);

    profile.experience.retain_mut(|e| {
        e.company = clean(&e.company);
        e.title = clean(&e.title);
        clean_opt(&mut e.location);
        clean_opt(&mut e.start);
        clean_opt(&mut e.end);
        clean_bullets(&mut e.bullets);
        !(e.company.is_empty() && e.title.is_empty() && e.bullets.is_empty())
    });
    profile.projects.retain_mut(|p| {
        p.name = clean(&p.name);
        clean_opt(&mut p.description);
        clean_bullets(&mut p.bullets);
        !(p.name.is_empty() && p.bullets.is_empty())
    });
    profile.education.retain_mut(|e| {
        e.school = clean(&e.school);
        clean_opt(&mut e.degree);
        clean_opt(&mut e.field);
        clean_opt(&mut e.start);
        clean_opt(&mut e.end);
        !e.school.is_empty()
    });
    let mut seen = HashSet::new();
    profile.skills.retain_mut(|s| {
        s.name = clean(&s.name);
        s.years = s.years.filter(|y| y.is_finite() && *y > 0.0 && *y <= 60.0);
        clean_list(&mut s.used_at);
        !s.name.is_empty() && seen.insert(s.name.to_lowercase())
    });
    clean_list(&mut profile.certifications);
    clean_list(&mut profile.languages);
    clean_list(&mut profile.awards);
}

pub fn assign_ids(profile: &mut ResumeProfile) {
    fn next_free(used: &HashSet<String>, prefix: &str) -> String {
        (1..)
            .map(|n| format!("{prefix}{n}"))
            .find(|id| !used.contains(id))
            .unwrap()
    }
    fn fill_bullets(owner: &str, bullets: &mut [Bullet]) {
        let mut used: HashSet<String> = bullets.iter().map(|b| b.id.clone()).collect();
        for b in bullets
            .iter_mut()
            .filter(|b| b.id.is_empty() || !b.id.starts_with(&format!("{owner}.b")))
        {
            b.id = next_free(&used, &format!("{owner}.b"));
            used.insert(b.id.clone());
        }
    }

    let mut used: HashSet<String> = HashSet::new();
    for e in &mut profile.experience {
        if e.id.is_empty() || !used.insert(e.id.clone()) {
            e.id = next_free(&used, "e");
            used.insert(e.id.clone());
        }
        fill_bullets(&e.id, &mut e.bullets);
    }
    for p in &mut profile.projects {
        if p.id.is_empty() || !used.insert(p.id.clone()) {
            p.id = next_free(&used, "p");
            used.insert(p.id.clone());
        }
        fill_bullets(&p.id, &mut p.bullets);
    }
    for e in &mut profile.education {
        if e.id.is_empty() || !used.insert(e.id.clone()) {
            e.id = next_free(&used, "ed");
            used.insert(e.id.clone());
        }
    }
}

pub fn check_against(profile: &ResumeProfile, resume_text: &str) -> Vec<Warning> {
    let source = normalize(resume_text);
    if source.is_empty() {
        return Vec::new();
    }
    profile
        .experience
        .iter()
        .flat_map(|e| &e.bullets)
        .chain(profile.projects.iter().flat_map(|p| &p.bullets))
        .filter(|b| !source.contains(&normalize(&b.text)))
        .map(|b| Warning {
            item_id: b.id.clone(),
            message: "Not found word for word in your resume.".into(),
        })
        .collect()
}
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub async fn read_resume<M: JsonModel>(
    model: &M,
    resume_text: &str,
) -> Result<(ResumeProfile, Vec<Warning>), AiError> {
    let user = format!("Resume text:\n<<<\n{resume_text}\n>>>");
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
        match serde_json::from_value::<RawResume>(value) {
            Ok(raw) => {
                let mut profile = ResumeProfile::from(raw);
                tidy(&mut profile);
                assign_ids(&mut profile);
                if profile.contact.name.is_empty() && profile.experience.is_empty() {
                    last_error = Some(AiError::BadOutput("no name or experience found".into()));
                    continue;
                }
                let warnings = check_against(&profile, resume_text);
                return Ok((profile, warnings));
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

    const RESUME_TEXT: &str = "Jordan Rivera\njordan@example.com | Seattle, WA\n\nEXPERIENCE\nSenior Software Engineer, Acme Corp (2021 - Present)\n- Led migration of billing services to Rust, cutting p99 latency by 40%\n- Mentored 4 engineers\n\nSKILLS\nRust, TypeScript";

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

    fn answer() -> Value {
        json!({
            "contact": { "name": " Jordan  Rivera ", "email": "jordan@example.com", "phone": null, "location": "Seattle, WA", "links": [] },
            "summary": null,
            "headline": "Senior engineer working in Rust",
            "experience": [{
                "company": "Acme Corp", "title": "Senior Software Engineer", "location": null,
                "start": "2021", "end": "Present",
                "bullets": [
                    "• Led migration of billing services to Rust, cutting p99 latency by 40%",
                    "Mentored 4 engineers",
                    "Built a world-class platform from scratch",
                    "  "
                ]
            }],
            "projects": [],
            "education": [],
            "skills": [
                { "name": "Rust", "years": 5, "used_at": ["Acme Corp"] },
                { "name": "rust", "years": null, "used_at": [] },
                { "name": "TypeScript", "years": -1, "used_at": [] }
            ],
            "certifications": [], "languages": [], "awards": []
        })
    }

    #[tokio::test]
    async fn reads_tidies_assigns_ids_and_flags_invented_bullets() {
        let model = FakeModel {
            answers: Mutex::new(vec![Ok(answer())]),
        };
        let (profile, warnings) = read_resume(&model, RESUME_TEXT).await.unwrap();

        assert_eq!(profile.contact.name, "Jordan Rivera");
        let job = &profile.experience[0];
        assert_eq!(job.id, "e1");
        let ids: Vec<&str> = job.bullets.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, vec!["e1.b1", "e1.b2", "e1.b3"]);
        assert_eq!(
            job.bullets[0].text,
            "Led migration of billing services to Rust, cutting p99 latency by 40%"
        );
        assert_eq!(profile.skills.len(), 2);
        assert_eq!(profile.skills[1].years, None);

        assert_eq!(
            warnings,
            vec![Warning {
                item_id: "e1.b3".into(),
                message: "Not found word for word in your resume.".into()
            }]
        );
    }

    #[tokio::test]
    async fn retries_once_on_bad_output() {
        let model = FakeModel {
            answers: Mutex::new(vec![
                Err(AiError::BadOutput("cut off".into())),
                Ok(answer()),
            ]),
        };
        assert!(read_resume(&model, RESUME_TEXT).await.is_ok());

        let model = FakeModel {
            answers: Mutex::new(vec![
                Err(AiError::BadOutput("a".into())),
                Err(AiError::BadOutput("b".into())),
            ]),
        };
        assert!(matches!(
            read_resume(&model, RESUME_TEXT).await,
            Err(AiError::BadOutput(_))
        ));
    }

    #[tokio::test]
    async fn connection_errors_are_not_retried() {
        let model = FakeModel {
            answers: Mutex::new(vec![Err(AiError::Unreachable("x".into()))]),
        };
        assert!(matches!(
            read_resume(&model, RESUME_TEXT).await,
            Err(AiError::Unreachable(_))
        ));
    }

    #[test]
    fn assign_ids_keeps_existing_and_fills_new() {
        let mut profile = ResumeProfile {
            experience: vec![Experience {
                id: "e1".into(),
                bullets: vec![
                    Bullet {
                        id: "e1.b1".into(),
                        text: "a".into(),
                    },
                    Bullet {
                        id: String::new(),
                        text: "new".into(),
                    },
                    Bullet {
                        id: "e1.b3".into(),
                        text: "c".into(),
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        profile.experience.push(Experience {
            bullets: vec![Bullet {
                id: String::new(),
                text: "x".into(),
            }],
            ..Default::default()
        });
        assign_ids(&mut profile);
        let first: Vec<&str> = profile.experience[0]
            .bullets
            .iter()
            .map(|b| b.id.as_str())
            .collect();
        assert_eq!(first, vec!["e1.b1", "e1.b2", "e1.b3"]);
        assert_eq!(profile.experience[1].id, "e2");
        assert_eq!(profile.experience[1].bullets[0].id, "e2.b1");
    }

    #[test]
    fn verbatim_check_ignores_pdf_line_breaks() {
        let profile = ResumeProfile {
            experience: vec![Experience {
                bullets: vec![Bullet {
                    id: "e1.b1".into(),
                    text: "Mentored 4 engineers".into(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(check_against(&profile, "- Mentored 4\nengineers.").is_empty());
        assert!(check_against(&profile, "").is_empty());
    }
}
