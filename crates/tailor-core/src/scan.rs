//! One pass over every saved search: fetch from LinkedIn, filter locally,
//! store matches. Requests are spaced out and capped to stay polite.

use std::{collections::HashSet, future::Future, time::Duration};

use rand::Rng;

use crate::{
    linkedin::{JobCard, JobDetail, LinkedInClient, LinkedInError, SearchQuery},
    matching::{infer_work_mode, keyword_hits, title_matches},
    search::JobSearch,
    store::{Store, StoreError},
};

/// Where job listings come from. Implemented by [`LinkedInClient`]; tests use a fake.
pub trait JobSource {
    fn search(&self, query: &SearchQuery<'_>) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send;
    fn job_detail(&self, linkedin_id: &str) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send;
}

impl JobSource for LinkedInClient {
    fn search(&self, query: &SearchQuery<'_>) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send {
        LinkedInClient::search(self, query)
    }

    fn job_detail(&self, linkedin_id: &str) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send {
        LinkedInClient::job_detail(self, linkedin_id)
    }
}

#[derive(Debug, Clone)]
pub struct ScanLimits {
    /// Result requests per title. The first returns ~60 jobs; each extra
    /// "load more" page ~10, and signed-out visitors only get a few of those.
    pub max_pages_per_query: usize,
    /// Job descriptions fetched per scan; the rest wait for the next scan.
    /// LinkedIn starts refusing back-to-back job pages after a handful.
    pub max_details_per_run: usize,
    /// Random pause before every request, between these bounds.
    pub min_delay: Duration,
    pub max_delay: Duration,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_pages_per_query: 1,
            max_details_per_run: 10,
            min_delay: Duration::from_secs(3),
            max_delay: Duration::from_secs(8),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanReport {
    pub new_matches: usize,
    pub requests: usize,
    /// Matching titles still waiting for their description (budget ran
    /// out, or LinkedIn paused job pages).
    pub pending_details: usize,
    /// LinkedIn refused a job page, so description fetching stopped early.
    pub details_paused: bool,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum ScanError {
    #[error(transparent)]
    Source(#[from] LinkedInError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub async fn run_scan<S: JobSource>(store: &Store, source: &S, limits: &ScanLimits) -> ScanReport {
    let mut report = ScanReport::default();
    let searches = match store.list_searches() {
        Ok(searches) => searches,
        Err(e) => {
            report.error = Some(e.to_string());
            return report;
        }
    };

    let mut detail_budget = limits.max_details_per_run;
    for search in &searches {
        if let Err(e) = scan_search(store, source, limits, search, &mut detail_budget, &mut report).await {
            let blocked = matches!(e, ScanError::Source(LinkedInError::Blocked(_)));
            report.error = Some(e.to_string());
            if blocked {
                break; // Keep hammering a rate limit and it gets longer.
            }
        }
    }
    report
}

async fn scan_search<S: JobSource>(
    store: &Store,
    source: &S,
    limits: &ScanLimits,
    search: &JobSearch,
    detail_budget: &mut usize,
    report: &mut ScanReport,
) -> Result<(), ScanError> {
    let criteria = &search.criteria;
    // LinkedIn has no title field; search each title as keywords, or all
    // keywords together when there are no titles.
    let queries = if criteria.titles.is_empty() { vec![criteria.keywords.join(" ")] } else { criteria.titles.clone() };

    let mut seen = HashSet::new();
    let mut candidates: Vec<JobCard> = Vec::new();
    for keywords in &queries {
        let mut start = 0;
        for page in 0..limits.max_pages_per_query {
            pause(limits).await;
            report.requests += 1;
            let query = SearchQuery {
                keywords,
                location: criteria.location.as_deref(),
                posted_within: criteria.posted_within,
                work_modes: &criteria.work_modes,
                start,
            };
            let cards = match source.search(&query).await {
                Ok(cards) => cards,
                // Paging past the last result returns an error status, and
                // signed-out paging is capped with a 429. Either way this
                // query is done; keep what we have and move on.
                Err(LinkedInError::Status(_) | LinkedInError::Blocked(_)) if page > 0 => break,
                Err(e) => return Err(e.into()),
            };
            if cards.is_empty() {
                break;
            }
            start += cards.len();
            for card in cards {
                let wanted = criteria.titles.is_empty() || title_matches(&card.title, &criteria.titles);
                if wanted && seen.insert(card.linkedin_id.clone()) {
                    candidates.push(card);
                }
            }
        }
    }

    for card in candidates {
        let mut description = store.known_job(&card.linkedin_id)?.and_then(|job| job.description);
        let mut detail = None;
        if description.is_none() {
            if *detail_budget == 0 {
                report.pending_details += 1;
            } else {
                *detail_budget -= 1;
                pause(limits).await;
                report.requests += 1;
                match source.job_detail(&card.linkedin_id).await {
                    Ok(fetched) => {
                        detail = fetched;
                        description = detail.as_ref().map(|d| d.description.clone());
                    }
                    // Expected now and then: stop fetching descriptions for
                    // this scan, keep searching. The next scan picks up here.
                    Err(LinkedInError::Blocked(_)) => {
                        *detail_budget = 0;
                        report.details_paused = true;
                        report.pending_details += 1;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }

        let work_mode = infer_work_mode(&card.title, &card.location, description.as_deref());
        let job_id = store.upsert_job(&card, detail.as_ref(), work_mode)?;

        // Keyword and work-mode checks need the description; jobs still
        // without one are re-checked on the next scan.
        let Some(description) = description else { continue };
        if work_mode.is_some_and(|mode| !criteria.work_modes.contains(&mode)) {
            continue;
        }
        let hits = keyword_hits(&format!("{}\n{description}", card.title), &criteria.keywords);
        if !criteria.keywords.is_empty() && hits.is_empty() {
            continue;
        }
        if store.add_match(search.id, job_id, &hits)? {
            report.new_matches += 1;
        }
    }
    Ok(())
}

async fn pause(limits: &ScanLimits) {
    let delay = jitter(limits);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
}

fn jitter(limits: &ScanLimits) -> Duration {
    if limits.max_delay <= limits.min_delay {
        return limits.min_delay;
    }
    let ms = rand::thread_rng().gen_range(limits.min_delay.as_millis() as u64..=limits.max_delay.as_millis() as u64);
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;
    use crate::search::{PostedWithin, SearchInput, WorkMode};

    #[derive(Default)]
    struct FakeSource {
        /// keywords -> pages of results
        pages: HashMap<String, Vec<Vec<JobCard>>>,
        details: HashMap<String, String>,
        block_details: bool,
        block_after_first_page: bool,
        detail_calls: Mutex<usize>,
    }

    impl JobSource for FakeSource {
        fn search(&self, query: &SearchQuery<'_>) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send {
            let pages = self.pages.get(query.keywords).cloned().unwrap_or_default();
            // 2 cards per page in these fixtures.
            let result = if self.block_after_first_page && query.start > 0 {
                Err(LinkedInError::Blocked(429))
            } else {
                Ok(pages.get(query.start / 2).cloned().unwrap_or_default())
            };
            async move { result }
        }

        fn job_detail(&self, id: &str) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send {
            *self.detail_calls.lock().unwrap() += 1;
            let result = if self.block_details {
                Err(LinkedInError::Blocked(999))
            } else {
                Ok(self.details.get(id).map(|d| JobDetail {
                    description: d.clone(),
                    seniority: None,
                    employment_type: None,
                    industries: None,
                }))
            };
            async move { result }
        }
    }

    fn card(id: &str, title: &str) -> JobCard {
        JobCard {
            linkedin_id: id.into(),
            title: title.into(),
            company: "Acme".into(),
            company_url: None,
            location: "Dubai".into(),
            posted_on: None,
            url: format!("https://www.linkedin.com/jobs/view/{id}"),
        }
    }

    fn fast() -> ScanLimits {
        ScanLimits { max_pages_per_query: 3, max_details_per_run: 10, min_delay: Duration::ZERO, max_delay: Duration::ZERO }
    }

    fn setup(modes: Vec<WorkMode>) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "tailor-scan-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let store = Store::open(&dir).unwrap();
        let criteria = SearchInput {
            name: None,
            titles: vec!["Backend Engineer".into()],
            keywords: vec!["Rust".into()],
            location: Some("Dubai".into()),
            work_modes: modes,
            posted_within: PostedWithin::Week,
        }
        .validate()
        .unwrap();
        store.create_search(&criteria).unwrap();
        (store, dir)
    }

    fn source() -> FakeSource {
        let mut source = FakeSource::default();
        source.pages.insert(
            "Backend Engineer".into(),
            vec![
                vec![card("1", "Senior Backend Engineer"), card("2", "iOS Developer")],
                vec![card("3", "Backend Engineer (Remote)"), card("4", "Backend Engineer, Payments")],
            ],
        );
        source.details.insert("1".into(), "Hybrid role. We use Rust.".into());
        source.details.insert("3".into(), "Rust everywhere.".into());
        source.details.insert("4".into(), "Java shop.".into());
        source
    }

    #[tokio::test]
    async fn filters_by_title_keywords_and_work_mode() {
        let (store, dir) = setup(vec![WorkMode::Hybrid, WorkMode::Onsite]);
        let src = source();
        let report = run_scan(&store, &src, &fast()).await;

        assert_eq!(report.error, None);
        // "iOS Developer" never gets a detail fetch: title filter runs first.
        assert_eq!(*src.detail_calls.lock().unwrap(), 3);
        // 1 matches. 3 is remote (excluded), 4 lacks the keyword.
        let jobs = store.list_jobs(None).unwrap();
        assert_eq!(jobs.iter().map(|j| j.linkedin_id.as_str()).collect::<Vec<_>>(), vec!["1"]);
        assert_eq!(jobs[0].matched_keywords, vec!["Rust"]);
        assert_eq!(jobs[0].work_mode, Some(WorkMode::Hybrid));
        assert_eq!(report.new_matches, 1);

        // A second scan reuses stored descriptions and finds nothing new.
        let again = run_scan(&store, &src, &fast()).await;
        assert_eq!(*src.detail_calls.lock().unwrap(), 3);
        assert_eq!(again.new_matches, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn detail_budget_defers_to_next_scan() {
        let (store, dir) = setup(vec![WorkMode::Remote, WorkMode::Hybrid]);
        let src = source();
        let limits = ScanLimits { max_details_per_run: 1, ..fast() };

        let first = run_scan(&store, &src, &limits).await;
        assert_eq!((first.new_matches, first.pending_details), (1, 2));
        let second = run_scan(&store, &src, &limits).await;
        assert_eq!((second.new_matches, second.pending_details), (1, 1));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn paging_cap_keeps_first_page() {
        let (store, dir) = setup(vec![WorkMode::Hybrid, WorkMode::Onsite]);
        let src = FakeSource { block_after_first_page: true, ..source() };
        let report = run_scan(&store, &src, &fast()).await;
        assert_eq!(report.error, None);
        assert_eq!(report.new_matches, 1); // job 1 from page one
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn blocked_job_pages_pause_softly() {
        let (store, dir) = setup(vec![WorkMode::Hybrid]);
        let src = FakeSource { block_details: true, ..source() };
        let report = run_scan(&store, &src, &fast()).await;
        assert_eq!(report.error, None);
        assert!(report.details_paused);
        // One refused request, then no more attempts; all 3 candidates wait.
        assert_eq!(*src.detail_calls.lock().unwrap(), 1);
        assert_eq!(report.pending_details, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn blocked_search_stops_the_scan() {
        let (store, dir) = setup(vec![WorkMode::Hybrid]);

        struct AlwaysBlocked;
        impl JobSource for AlwaysBlocked {
            fn search(&self, _: &SearchQuery<'_>) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send {
                async { Err(LinkedInError::Blocked(429)) }
            }
            fn job_detail(&self, _: &str) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send {
                async { Ok(None) }
            }
        }
        let report = run_scan(&store, &AlwaysBlocked, &fast()).await;
        assert!(report.error.unwrap().contains("rate-limiting"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
