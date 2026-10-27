use std::{collections::HashMap, sync::Arc};

use tokio::{
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
};

use crate::{
    score_stage::ScoreMessage,
    db::Db,
    errors::DbError,
    scrape_stage::{ScanReport, ScrapedJob},
    types::{JobSearch, WorkMode},
};

pub enum MatchMessage {
    Batch {
        run_id: i64,
        search: JobSearch,
        jobs: Vec<ScrapedJob>,
    },
    RunFinished {
        run_id: i64,
        report: ScanReport,
    },
    Flush {
        reply: oneshot::Sender<()>,
    },
}

pub struct MatchStage {
    handle: JoinHandle<()>,
}

impl MatchStage {
    pub fn new(db: Arc<Db>, rx: UnboundedReceiver<MatchMessage>, scorer: UnboundedSender<ScoreMessage>) -> Self {
        let handle = tokio::spawn(run(db, rx, scorer));
        Self { handle }
    }

    pub async fn join(self) {
        let _ = self.handle.await;
    }
}

async fn run(db: Arc<Db>, mut rx: UnboundedReceiver<MatchMessage>, scorer: UnboundedSender<ScoreMessage>) {
    let mut new_matches: HashMap<i64, usize> = HashMap::new();
    let mut match_errors: HashMap<i64, String> = HashMap::new();
    while let Some(message) = rx.recv().await {
        match message {
            MatchMessage::Batch {
                run_id,
                search,
                jobs,
            } => {
                let db = db.clone();
                match tokio::task::spawn_blocking(move || apply_batch(&db, &search, jobs)).await {
                    Ok(Ok(count)) => {
                        *new_matches.entry(run_id).or_default() += count;
                        let _ = scorer.send(ScoreMessage::Kick);
                    }
                    Ok(Err(error)) => {
                        tracing::error!("couldn't match scanned jobs: {error}");
                        match_errors.insert(run_id, error.to_string());
                    }
                    Err(error) => {
                        let error = DbError::Io(std::io::Error::other(error.to_string()));
                        tracing::error!("couldn't match scanned jobs: {error}");
                        match_errors.insert(run_id, error.to_string());
                    }
                }
            }
            MatchMessage::RunFinished { run_id, mut report } => {
                report.new_matches = new_matches.remove(&run_id).unwrap_or_default();
                if let Some(error) = match_errors.remove(&run_id) {
                    report.error = Some(error);
                }
                finish_run(&db, run_id, &report).await;
            }
            MatchMessage::Flush { reply } => {
                let _ = reply.send(());
            }
        }
    }
}

pub(crate) fn apply_batch(
    db: &Db,
    search: &JobSearch,
    jobs: Vec<ScrapedJob>,
) -> Result<usize, DbError> {
    let mut new_matches = 0;
    for job in jobs {
        let ScrapedJob {
            card,
            detail,
            description,
            work_mode,
        } = job;
        let work_mode = work_mode
            .or_else(|| infer_work_mode(&card.title, &card.location, description.as_deref()));
        let job_id = db.upsert_job(&card, detail.as_ref(), work_mode)?;
        let text = format!(
            "{}\n{}",
            card.title,
            description.as_deref().unwrap_or_default()
        );
        let hits = keyword_hits(&text, &search.criteria.keywords);
        let keywords_ok = search.criteria.keywords.is_empty() || !hits.is_empty();
        let wrong_mode = work_mode.is_some_and(|mode| !search.criteria.work_modes.contains(&mode));
        if wrong_mode || (!keywords_ok && description.is_some()) {
            db.remove_match(search.id, job_id)?;
            continue;
        }
        if !keywords_ok {
            continue;
        }
        if db.add_match(search.id, job_id, &hits)? {
            new_matches += 1;
        }
    }
    Ok(new_matches)
}

async fn finish_run(db: &Arc<Db>, run_id: i64, report: &ScanReport) {
    let status = match (&report.error, report.requests) {
        (None, _) => "ok",
        (Some(_), n) if n > 1 => "partial",
        (Some(_), _) => "failed",
    };
    let mut notes: Vec<String> = Vec::new();
    if let Some(error) = &report.error {
        notes.push(if error.ends_with('.') {
            error.clone()
        } else {
            format!("{error}.")
        });
    }
    if report.details_paused {
        notes.push(format!(
            "LinkedIn paused job page downloads; {} jobs will be completed next scan.",
            report.pending_details
        ));
    } else if report.pending_details > 0 {
        notes.push(format!(
            "{} jobs still need details; continuing next scan.",
            report.pending_details
        ));
    }
    for location in &report.skipped_locations {
        notes.push(format!(
            "Indeed was skipped for \"{location}\" because the country couldn't be worked out."
        ));
    }
    let message = (!notes.is_empty()).then(|| notes.join(" "));
    tracing::info!(
        status,
        new_matches = report.new_matches,
        requests = report.requests,
        "scan finished"
    );
    let db = db.clone();
    let new_matches = report.new_matches;
    match tokio::task::spawn_blocking(move || {
        db.finish_scan_run(run_id, status, new_matches, message.as_deref())
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!("couldn't record scan result: {error}"),
        Err(error) => tracing::error!("couldn't record scan result: {error}"),
    }
}

pub fn contains_term(haystack: &str, term: &str) -> bool {
    let haystack = haystack.to_lowercase();
    let term = term.trim().to_lowercase();
    let Some(first) = term.chars().next() else {
        return false;
    };

    let mut from = 0;
    while let Some(offset) = haystack[from..].find(&term) {
        let begin = from + offset;
        let end = begin + term.len();
        let clear_before = haystack[..begin]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let clear_after = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if clear_before && clear_after {
            return true;
        }
        from = begin + first.len_utf8();
    }
    false
}

pub fn title_matches(job_title: &str, titles: &[String]) -> bool {
    titles.iter().any(|title| {
        title
            .split_whitespace()
            .all(|word| contains_term(job_title, word))
    })
}

pub fn keyword_hits(text: &str, keywords: &[String]) -> Vec<String> {
    keywords
        .iter()
        .filter(|k| contains_term(text, k))
        .cloned()
        .collect()
}

const TITLE_SIGNALS: &[(WorkMode, &[&str])] = &[
    (WorkMode::Remote, &["remote", "work from home", "wfh"]),
    (WorkMode::Hybrid, &["hybrid"]),
    (
        WorkMode::Onsite,
        &["on-site", "onsite", "on site", "in-office", "office-based"],
    ),
];

const DESCRIPTION_SIGNALS: &[(WorkMode, &[&str])] = &[
    (
        WorkMode::Remote,
        &[
            "fully remote",
            "100% remote",
            "remote-first",
            "remote first",
            "remote position",
            "remote role",
            "work remotely",
            "work from home",
            "remote work",
        ],
    ),
    (WorkMode::Hybrid, &["hybrid"]),
    (
        WorkMode::Onsite,
        &[
            "on-site",
            "onsite",
            "in-office",
            "office-based",
            "work from office",
            "work from the office",
        ],
    ),
];

fn modes_in(text: &str, signals: &[(WorkMode, &[&str])]) -> Vec<WorkMode> {
    signals
        .iter()
        .filter(|(_, phrases)| phrases.iter().any(|p| contains_term(text, p)))
        .map(|(mode, _)| *mode)
        .collect()
}

pub fn infer_work_mode(title: &str, location: &str, description: Option<&str>) -> Option<WorkMode> {
    let pick = |modes: Vec<WorkMode>| match modes.as_slice() {
        [only] => Some(*only),
        many if many.contains(&WorkMode::Hybrid) => Some(WorkMode::Hybrid),
        _ => None,
    };

    let headline = modes_in(&format!("{title} {location}"), TITLE_SIGNALS);
    if !headline.is_empty() {
        return pick(headline);
    }
    description.and_then(|d| pick(modes_in(d, DESCRIPTION_SIGNALS)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_term_matching() {
        assert!(contains_term("Senior Java Developer", "java"));
        assert!(!contains_term("JavaScript Developer", "Java"));
        assert!(contains_term("Senior C++ Engineer", "C++"));
        assert!(contains_term("Sr .NET Developer", ".net"));
        assert!(!contains_term("anything", "  "));
    }

    #[test]
    fn titles_match_all_words_any_order() {
        let titles = vec!["Backend Engineer".to_string()];
        assert!(title_matches(
            "Senior Backend Engineer (IOT & Messaging)",
            &titles
        ));
        assert!(title_matches("Engineer, Backend", &titles));
        assert!(!title_matches("Senior Backend Developer", &titles));
    }

    #[test]
    fn keyword_hits_in_order() {
        let keywords = vec!["Rust".to_string(), "Kafka".to_string(), "Go".to_string()];
        assert_eq!(
            keyword_hits("We use Kafka and Rust", &keywords),
            vec!["Rust", "Kafka"]
        );
    }

    #[test]
    fn work_mode_from_title_or_location() {
        assert_eq!(
            infer_work_mode("Fullstack Engineer (100% Remote)", "UAE", None),
            Some(WorkMode::Remote)
        );
        assert_eq!(
            infer_work_mode("Engineer", "Dubai (Hybrid)", None),
            Some(WorkMode::Hybrid)
        );
    }

    #[test]
    fn work_mode_from_description_needs_strong_phrasing() {
        assert_eq!(
            infer_work_mode("Engineer", "Dubai", Some("You'll work with remote teams.")),
            None
        );
        assert_eq!(
            infer_work_mode("Engineer", "Dubai", Some("This is a fully remote role.")),
            Some(WorkMode::Remote)
        );
        assert_eq!(
            infer_work_mode("Engineer", "Dubai", Some("Hybrid: 3 days in-office.")),
            Some(WorkMode::Hybrid)
        );
        assert_eq!(
            infer_work_mode("Engineer", "Dubai", Some("Based on-site in Abu Dhabi.")),
            Some(WorkMode::Onsite)
        );
        assert_eq!(infer_work_mode("Engineer", "Dubai", None), None);
    }
}
