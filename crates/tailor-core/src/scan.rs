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
    /// "Load more" pages (10 jobs each) per scan, shared by all titles; each
    /// title's first request is the full page (~60 jobs). Signed-out visitors
    /// get about three per window, and a refused one ends paging for the scan.
    pub max_load_more_per_run: usize,
    /// Job descriptions fetched per scan; the rest wait for the next scan.
    /// LinkedIn starts refusing back-to-back job pages after a handful.
    pub max_details_per_run: usize,
    /// Random pause before every request, between these bounds. Bursts are
    /// what trip LinkedIn's limits.
    pub min_delay: Duration,
    pub max_delay: Duration,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_load_more_per_run: 2,
            max_details_per_run: 10,
            min_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(12),
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
    /// LinkedIn refused a results page, so remaining titles were skipped.
    /// Jobs already collected were still processed.
    pub search_blocked: bool,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum ScanError {
    #[error(transparent)]
    Source(#[from] LinkedInError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Requests left in this scan.
struct Budget {
    load_more: usize,
    details: usize,
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

    let mut budget = Budget { load_more: limits.max_load_more_per_run, details: limits.max_details_per_run };
    for search in &searches {
        if report.search_blocked {
            break; // Keep hammering a rate limit and it gets longer.
        }
        if let Err(e) = scan_search(store, source, limits, search, &mut budget, &mut report).await {
            report.error = Some(e.to_string());
        }
    }
    report
}

async fn scan_search<S: JobSource>(
    store: &Store,
    source: &S,
    limits: &ScanLimits,
    search: &JobSearch,
    budget: &mut Budget,
    report: &mut ScanReport,
) -> Result<(), ScanError> {
    let criteria = &search.criteria;
    // LinkedIn has no title field; search each title as keywords, or all
    // keywords together when there are no titles.
    let queries = if criteria.titles.is_empty() { vec![criteria.keywords.join(" ")] } else { criteria.titles.clone() };

    let mut seen = HashSet::new();
    let mut candidates: Vec<JobCard> = Vec::new();
    'queries: for keywords in &queries {
        let mut start = 0;
        loop {
            let first_page = start == 0;
            if !first_page && budget.load_more == 0 {
                break;
            }
            pause(limits).await;
            report.requests += 1;
            let query = SearchQuery {
                keywords,
                location: criteria.location.as_deref(),
                posted_within: criteria.posted_within,
                work_modes: &criteria.work_modes,
                start,
            };
            let result = source.search(&query).await;
            if !first_page {
                budget.load_more -= 1;
            }
            let cards = match result {
                Ok(cards) => cards,
                // Signed-out paging cap: no more "load more" this scan.
                Err(LinkedInError::Blocked(_)) if !first_page => {
                    budget.load_more = 0;
                    break;
                }
                // Paging past the last result.
                Err(LinkedInError::Status(_)) if !first_page => break,
                // A results page failed: stop searching, but still process
                // the jobs already collected below.
                Err(e) => {
                    report.search_blocked |= matches!(e, LinkedInError::Blocked(_));
                    report.error = Some(e.to_string());
                    break 'queries;
                }
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
            if budget.details == 0 {
                report.pending_details += 1;
            } else {
                budget.details -= 1;
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
                        budget.details = 0;
                        report.details_paused = true;
                        report.pending_details += 1;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }

        let work_mode = infer_work_mode(&card.title, &card.location, description.as_deref());
        let job_id = store.upsert_job(&card, detail.as_ref(), work_mode)?;

        // List jobs as soon as the title matches; the description only
        // matters for keywords and work mode. Re-checked every scan, so a
        // description that arrives later can still rule a job out.
        let text = format!("{}\n{}", card.title, description.as_deref().unwrap_or_default());
        let hits = keyword_hits(&text, &criteria.keywords);
        let keywords_ok = criteria.keywords.is_empty() || !hits.is_empty();
        let wrong_mode = work_mode.is_some_and(|mode| !criteria.work_modes.contains(&mode));
        if wrong_mode || (!keywords_ok && description.is_some()) {
            store.remove_match(search.id, job_id)?;
            continue;
        }
        if !keywords_ok {
            continue; // Keywords may still be in the description; check once it's fetched.
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
        /// Queries whose results page is refused outright.
        blocked_queries: Vec<String>,
        detail_calls: Mutex<usize>,
    }

    impl JobSource for FakeSource {
        fn search(&self, query: &SearchQuery<'_>) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send {
            let pages = self.pages.get(query.keywords).cloned().unwrap_or_default();
            // 2 cards per page in these fixtures.
            let blocked = self.blocked_queries.iter().any(|q| q == query.keywords);
            let result = if blocked || (self.block_after_first_page && query.start > 0) {
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
        ScanLimits { max_load_more_per_run: 2, max_details_per_run: 10, min_delay: Duration::ZERO, max_delay: Duration::ZERO }
    }

    fn setup(modes: Vec<WorkMode>) -> (Store, std::path::PathBuf) {
        setup_with(vec!["Backend Engineer".into()], modes)
    }

    fn setup_with(titles: Vec<String>, modes: Vec<WorkMode>) -> (Store, std::path::PathBuf) {
        let dir = crate::store::test_dir("tailor-scan-test");
        let store = Store::open(&dir).unwrap();
        let criteria = SearchInput {
            name: None,
            titles,
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
    async fn blocked_results_page_keeps_collected_jobs() {
        let titles = vec!["Backend Engineer".into(), "Platform Engineer".into()];
        let (store, dir) = setup_with(titles, vec![WorkMode::Hybrid, WorkMode::Onsite]);
        let src = FakeSource { blocked_queries: vec!["Platform Engineer".into()], ..source() };
        let report = run_scan(&store, &src, &fast()).await;
        assert!(report.search_blocked);
        assert!(report.error.unwrap().contains("rate-limiting"));
        // The first title's jobs were still evaluated.
        assert_eq!(report.new_matches, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn load_more_budget_is_shared_across_titles() {
        let titles = vec!["Backend Engineer".into(), "Engineer".into()];
        let (store, dir) = setup_with(titles, vec![WorkMode::Hybrid]);
        let mut src = source();
        let pages = src.pages["Backend Engineer"].clone();
        src.pages.insert("Engineer".into(), pages);
        let limits = ScanLimits { max_load_more_per_run: 1, ..fast() };
        let report = run_scan(&store, &src, &limits).await;
        // Title 1: full page + 1 load more. Title 2: full page only.
        let searches = report.requests - *src.detail_calls.lock().unwrap();
        assert_eq!(searches, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn without_keywords_jobs_list_before_descriptions() {
        let (store, dir) = setup_with(vec!["Backend Engineer".into()], vec![WorkMode::Hybrid, WorkMode::Onsite]);
        store
            .update_search(store.list_searches().unwrap()[0].id, &{
                let mut c = store.list_searches().unwrap()[0].criteria.clone();
                c.keywords.clear();
                c
            })
            .unwrap();
        let src = source();
        let limits = ScanLimits { max_details_per_run: 0, ..fast() };
        let report = run_scan(&store, &src, &limits).await;
        // 1, 4 listed straight away; 3 is "(Remote)" in the title, so excluded.
        assert_eq!(report.new_matches, 2);
        assert_eq!(*src.detail_calls.lock().unwrap(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn later_description_can_remove_a_match() {
        let (store, dir) = setup_with(vec!["Backend Engineer".into()], vec![WorkMode::Remote]);
        let search = store.list_searches().unwrap()[0].clone();
        let mut criteria = search.criteria.clone();
        criteria.keywords.clear();
        store.update_search(search.id, &criteria).unwrap();

        let mut src = source();
        src.details.insert("4".into(), "Fully on-site in Dubai.".into());
        // First scan: no descriptions, so 1, 3, 4 are listed on title alone.
        run_scan(&store, &src, &ScanLimits { max_details_per_run: 0, ..fast() }).await;
        assert_eq!(store.list_jobs(None).unwrap().len(), 3);
        // Second scan fetches descriptions: 1 is hybrid, 4 on-site; only 3 stays.
        run_scan(&store, &src, &fast()).await;
        let ids: Vec<String> = store.list_jobs(None).unwrap().into_iter().map(|j| j.linkedin_id).collect();
        assert_eq!(ids, vec!["3"]);
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
