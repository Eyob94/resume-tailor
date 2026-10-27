use serde::{Deserialize, Serialize};

use crate::errors::InvalidSearch;

#[derive(Debug, Clone, Serialize)]
pub struct MasterResume {
    pub file_name: String,
    pub size_bytes: u64,
    pub page_count: usize,
    pub extracted_text: String,
    pub uploaded_at: String,
}

#[derive(Debug, Clone)]
pub struct PdfInfo {
    pub page_count: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ResumeProfile {
    pub contact: Contact,
    pub summary: Option<String>,
    pub headline: Option<String>,
    pub experience: Vec<Experience>,
    pub projects: Vec<Project>,
    pub education: Vec<Education>,
    pub skills: Vec<Skill>,
    pub certifications: Vec<String>,
    pub languages: Vec<String>,
    pub awards: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Contact {
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub location: Option<String>,
    pub links: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Experience {
    pub id: String,
    pub company: String,
    pub title: String,
    pub location: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub bullets: Vec<Bullet>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bullet {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub bullets: Vec<Bullet>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Education {
    pub id: String,
    pub school: String,
    pub degree: Option<String>,
    pub field: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Skill {
    pub name: String,
    pub years: Option<f32>,
    pub used_at: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    pub item_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkMode {
    Onsite,
    Hybrid,
    Remote,
}

impl WorkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Onsite => "onsite",
            Self::Hybrid => "hybrid",
            Self::Remote => "remote",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "onsite" => Some(Self::Onsite),
            "hybrid" => Some(Self::Hybrid),
            "remote" => Some(Self::Remote),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PostedWithin {
    Day,
    #[default]
    Week,
    Month,
}

impl PostedWithin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            _ => None,
        }
    }

    pub fn linkedin_param(self) -> &'static str {
        match self {
            Self::Day => "r86400",
            Self::Week => "r604800",
            Self::Month => "r2592000",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub titles: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub work_modes: Vec<WorkMode>,
    #[serde(default)]
    pub posted_within: PostedWithin,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchCriteria {
    pub name: String,
    pub titles: Vec<String>,
    pub keywords: Vec<String>,
    pub location: Option<String>,
    pub work_modes: Vec<WorkMode>,
    pub posted_within: PostedWithin,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSearch {
    pub id: i64,
    pub enabled: bool,
    #[serde(flatten)]
    pub criteria: SearchCriteria,
    pub created_at: String,
    pub updated_at: String,
}

const MAX_TERMS: usize = 20;
const MAX_TERM_CHARS: usize = 100;
const MAX_NAME_CHARS: usize = 80;

impl SearchInput {
    pub fn validate(self) -> Result<SearchCriteria, InvalidSearch> {
        let titles = clean_terms(self.titles, "job titles")?;
        let keywords = clean_terms(self.keywords, "keywords")?;
        if titles.is_empty() && keywords.is_empty() {
            return Err(InvalidSearch(
                "Add at least one job title or keyword.".into(),
            ));
        }

        let mut work_modes = self.work_modes;
        work_modes.sort();
        work_modes.dedup();
        if work_modes.is_empty() {
            return Err(InvalidSearch("Pick at least one work mode.".into()));
        }

        let location = self
            .location
            .map(|l| collapse_spaces(&l))
            .filter(|l| !l.is_empty());
        if location
            .as_ref()
            .is_some_and(|l| l.chars().count() > MAX_TERM_CHARS)
        {
            return Err(InvalidSearch(format!(
                "Location must be under {MAX_TERM_CHARS} characters."
            )));
        }
        if location.is_none() && work_modes.iter().any(|m| *m != WorkMode::Remote) {
            return Err(InvalidSearch(
                "Add a location for on-site or hybrid searches.".into(),
            ));
        }

        let name = self
            .name
            .map(|n| collapse_spaces(&n))
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(MAX_NAME_CHARS).collect())
            .unwrap_or_else(|| default_name(&titles, &keywords, location.as_deref()));

        Ok(SearchCriteria {
            name,
            titles,
            keywords,
            location,
            work_modes,
            posted_within: self.posted_within,
        })
    }
}

fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_terms(raw: Vec<String>, label: &str) -> Result<Vec<String>, InvalidSearch> {
    let mut terms: Vec<String> = Vec::new();
    for term in raw {
        let term = collapse_spaces(&term);
        if term.is_empty() {
            continue;
        }
        if term.chars().count() > MAX_TERM_CHARS {
            return Err(InvalidSearch(format!(
                "Each of the {label} must be under {MAX_TERM_CHARS} characters."
            )));
        }
        if !terms.iter().any(|t| t.eq_ignore_ascii_case(&term)) {
            terms.push(term);
        }
    }
    if terms.len() > MAX_TERMS {
        return Err(InvalidSearch(format!("Use at most {MAX_TERMS} {label}.")));
    }
    Ok(terms)
}

fn default_name(titles: &[String], keywords: &[String], location: Option<&str>) -> String {
    let what = titles
        .first()
        .or(keywords.first())
        .map(String::as_str)
        .unwrap_or("Search");
    format!("{what} · {}", location.unwrap_or("Remote"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    New,
    Saved,
    Dismissed,
    Applied,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Saved => "saved",
            Self::Dismissed => "dismissed",
            Self::Applied => "applied",
        }
    }

    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "saved" => Self::Saved,
            "dismissed" => Self::Dismissed,
            "applied" => Self::Applied,
            _ => Self::New,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JobListing {
    pub id: i64,
    pub linkedin_id: String,
    pub title: String,
    pub company: String,
    pub company_url: Option<String>,
    pub location: String,
    pub posted_on: Option<String>,
    pub url: String,
    pub description: Option<String>,
    pub seniority: Option<String>,
    pub employment_type: Option<String>,
    pub industries: Option<String>,
    pub work_mode: Option<WorkMode>,
    pub status: JobStatus,
    pub first_seen_at: String,
    pub tailored_at: Option<String>,
    pub search_ids: Vec<i64>,
    pub matched_keywords: Vec<String>,
    pub score: Option<JobScore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Importance {
    Must,
    Nice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Met,
    Partial,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    pub text: String,
    pub importance: Importance,
    pub verdict: Verdict,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobScore {
    pub status: String,
    pub score: Option<u8>,
    pub summary: Option<String>,
    pub requirements: Vec<Requirement>,
    pub model: String,
    pub error: Option<String>,
    pub profile_id: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct JobToScore {
    pub job_id: i64,
    pub title: String,
    pub company: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanRun {
    pub id: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub new_matches: i64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobCard {
    pub linkedin_id: String,
    pub title: String,
    pub company: String,
    pub company_url: Option<String>,
    pub location: String,
    pub posted_on: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobDetail {
    pub description: String,
    pub seniority: Option<String>,
    pub employment_type: Option<String>,
    pub industries: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaStatus {
    pub url: String,
    pub reachable: bool,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanStatus {
    pub running: bool,
    pub interval_minutes: u32,
    pub next_run_at: Option<String>,
    pub last_run: Option<ScanRun>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileRecord {
    pub id: i64,
    pub resume_uploaded_at: String,
    pub status: String,
    pub model: String,
    pub profile: Option<ResumeProfile>,
    pub warnings: Vec<Warning>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub confirmed_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> SearchInput {
        SearchInput {
            name: None,
            titles: vec![
                "  Backend   Engineer ".into(),
                "backend engineer".into(),
                "".into(),
            ],
            keywords: vec!["Rust".into()],
            location: Some(" Dubai ".into()),
            work_modes: vec![WorkMode::Remote, WorkMode::Hybrid, WorkMode::Remote],
            posted_within: PostedWithin::Week,
        }
    }

    #[test]
    fn normalizes_terms_and_builds_default_name() {
        let c = input().validate().unwrap();
        assert_eq!(c.titles, vec!["Backend Engineer"]);
        assert_eq!(c.location.as_deref(), Some("Dubai"));
        assert_eq!(c.work_modes, vec![WorkMode::Hybrid, WorkMode::Remote]);
        assert_eq!(c.name, "Backend Engineer · Dubai");
    }

    #[test]
    fn requires_title_or_keyword() {
        let mut i = input();
        i.titles.clear();
        i.keywords.clear();
        assert!(i.validate().is_err());
    }

    #[test]
    fn location_optional_only_for_remote() {
        let mut i = input();
        i.location = None;
        assert!(i.clone().validate().is_err());
        i.work_modes = vec![WorkMode::Remote];
        assert_eq!(i.validate().unwrap().name, "Backend Engineer · Remote");
    }

    #[test]
    fn requires_work_mode() {
        let mut i = input();
        i.work_modes.clear();
        assert!(i.validate().is_err());
    }
}
