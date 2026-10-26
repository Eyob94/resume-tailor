use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::Rng;
use scraper::{ElementRef, Html, Selector};
use tokio::{
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
};

use crate::{
    db::Db,
    errors::{DbError, LinkedInError, ScanError},
    match_stage::{title_matches, MatchMessage},
    types::{JobCard, JobDetail, JobSearch, PostedWithin, ScanStatus, WorkMode},
};

#[derive(Debug, Clone)]
pub struct ScrapedJob {
    pub card: JobCard,
    pub detail: Option<JobDetail>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanLimits {
    pub max_load_more_per_run: usize,
    pub max_details_per_run: usize,
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
    pub pending_details: usize,
    pub details_paused: bool,
    pub search_blocked: bool,
    pub error: Option<String>,
}

struct Budget {
    load_more: usize,
    details: usize,
}

pub trait JobSource {
    fn search(
        &self,
        query: &SearchQuery<'_>,
    ) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send;
    fn job_detail(
        &self,
        linkedin_id: &str,
    ) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send;
    fn company_website(
        &self,
        _company_url: &str,
    ) -> impl Future<Output = Result<Option<String>, LinkedInError>> + Send {
        async { Ok(None) }
    }
}

impl JobSource for LinkedInClient {
    fn search(
        &self,
        query: &SearchQuery<'_>,
    ) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send {
        LinkedInClient::search(self, query)
    }

    fn job_detail(
        &self,
        linkedin_id: &str,
    ) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send {
        LinkedInClient::job_detail(self, linkedin_id)
    }

    fn company_website(
        &self,
        company_url: &str,
    ) -> impl Future<Output = Result<Option<String>, LinkedInError>> + Send {
        LinkedInClient::company_website(self, company_url)
    }
}

pub enum ScrapeMessage {
    Status {
        reply: oneshot::Sender<Result<ScanStatus, DbError>>,
    },
    RunNow {
        reply: oneshot::Sender<Result<(), String>>,
    },
    Reschedule,
}

pub struct ScrapeStage {
    handle: JoinHandle<()>,
}

impl ScrapeStage {
    pub fn new(
        db: Arc<Db>,
        client: LinkedInClient,
        cooldown_seconds: i64,
        rx: UnboundedReceiver<ScrapeMessage>,
        matcher: UnboundedSender<MatchMessage>,
    ) -> Self {
        let handle = tokio::spawn(run(
            db,
            Arc::new(client),
            ChronoDuration::seconds(cooldown_seconds),
            rx,
            matcher,
        ));
        Self { handle }
    }

    pub async fn join(self) {
        let _ = self.handle.await;
    }
}

async fn run(
    db: Arc<Db>,
    client: Arc<LinkedInClient>,
    cooldown: ChronoDuration,
    mut rx: UnboundedReceiver<ScrapeMessage>,
    matcher: UnboundedSender<MatchMessage>,
) {
    let db_for_cleanup = db.clone();
    match tokio::task::spawn_blocking(move || db_for_cleanup.mark_interrupted_scan_runs()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!("couldn't tidy previous scan runs: {e}"),
        Err(e) => tracing::error!("couldn't tidy previous scan runs: {e}"),
    }

    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut running = false;
    let mut run_requested = false;
    let mut next_run_at: Option<DateTime<Utc>> = None;
    loop {
        if !running {
            next_run_at = compute_next_run(&db);
            let due = next_run_at.is_some_and(|time| time <= Utc::now());
            if (run_requested || due) && has_searches(&db) {
                run_requested = false;
                running = true;
                next_run_at = None;
                let (db, client, matcher, done_tx) =
                    (db.clone(), client.clone(), matcher.clone(), done_tx.clone());
                tokio::spawn(async move {
                    scrape_run(db, client, matcher).await;
                    let _ = done_tx.send(());
                });
                continue;
            }
            run_requested = false;
        }
        let wait = next_run_at.map(|time| (time - Utc::now()).to_std().unwrap_or_default());
        tokio::select! {
            message = rx.recv() => match message {
                None => break,
                Some(ScrapeMessage::Status { reply }) => {
                    let _ = reply.send(status(&db, running, next_run_at));
                }
                Some(ScrapeMessage::RunNow { reply }) => {
                    let result = check_run_now(&db, running, cooldown);
                    if result.is_ok() {
                        run_requested = true;
                    }
                    let _ = reply.send(result);
                }
                Some(ScrapeMessage::Reschedule) => {}
            },
            _ = done_rx.recv(), if running => {
                running = false;
            }
            _ = tokio::time::sleep(wait.unwrap_or_default()), if !running && wait.is_some() => {}
        }
    }
}

pub const MIN_INTERVAL_MINUTES: u32 = 1;
pub const MAX_INTERVAL_MINUTES: u32 = 60;
pub const DEFAULT_INTERVAL_MINUTES: u32 = 60;
pub const INTERVAL_KEY: &str = "scan_interval_minutes";
pub const DEFAULT_COOLDOWN_SECONDS: i64 = MIN_INTERVAL_MINUTES as i64 * 60;

pub fn interval_minutes(db: &Db) -> u32 {
    db.get_setting(INTERVAL_KEY)
        .ok()
        .flatten()
        .and_then(|value| value.parse().ok())
        .map(|minutes: u32| minutes.clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES))
        .unwrap_or(DEFAULT_INTERVAL_MINUTES)
}

fn has_searches(db: &Db) -> bool {
    db.list_searches()
        .map(|searches| searches.iter().any(|search| search.enabled))
        .unwrap_or(false)
}

fn compute_next_run(db: &Db) -> Option<DateTime<Utc>> {
    if !has_searches(db) {
        return None;
    }
    let interval = ChronoDuration::minutes(interval_minutes(db).into());
    let last_started = db
        .last_scan_run()
        .ok()
        .flatten()
        .and_then(|run| DateTime::parse_from_rfc3339(&run.started_at).ok())
        .map(|time| time.with_timezone(&Utc));
    Some(
        last_started
            .map(|time| time + interval)
            .unwrap_or_else(Utc::now),
    )
}

fn status(
    db: &Db,
    running: bool,
    next_run_at: Option<DateTime<Utc>>,
) -> Result<ScanStatus, DbError> {
    Ok(ScanStatus {
        running,
        interval_minutes: interval_minutes(db),
        next_run_at: next_run_at.map(|time| time.to_rfc3339()),
        last_run: db.last_scan_run()?,
    })
}

fn check_run_now(db: &Db, running: bool, cooldown: ChronoDuration) -> Result<(), String> {
    if running {
        return Err("A scan is already running.".into());
    }
    let last_started = db
        .last_scan_run()
        .ok()
        .flatten()
        .and_then(|run| DateTime::parse_from_rfc3339(&run.started_at).ok())
        .map(|time| time.with_timezone(&Utc));
    if let Some(last) = last_started {
        let ready_at = last + cooldown;
        if Utc::now() < ready_at {
            let wait = (ready_at - Utc::now()).num_seconds() + 1;
            let wait = if wait > 60 {
                format!("{} min", (wait + 59) / 60)
            } else {
                format!("{wait} s")
            };
            return Err(format!("A scan ran recently. Try again in {wait}."));
        }
    }
    Ok(())
}

async fn scrape_run<S: JobSource + Send + Sync + 'static>(
    db: Arc<Db>,
    source: Arc<S>,
    matcher: UnboundedSender<MatchMessage>,
) {
    let db_for_start = db.clone();
    let run_id = match tokio::task::spawn_blocking(move || db_for_start.start_scan_run()).await {
        Ok(Ok(id)) => id,
        Ok(Err(e)) => {
            tracing::error!("couldn't record scan start: {e}");
            return;
        }
        Err(e) => {
            tracing::error!("couldn't record scan start: {e}");
            return;
        }
    };
    tracing::info!("scan started");
    let report = scrape_searches(
        &db,
        source.as_ref(),
        &ScanLimits::default(),
        |search, jobs| {
            let _ = matcher.send(MatchMessage::Batch {
                search: search.clone(),
                jobs,
            });
        },
    )
    .await;
    let _ = matcher.send(MatchMessage::RunFinished { run_id, report });
    let (flushed_tx, flushed_rx) = oneshot::channel();
    if matcher.send(MatchMessage::Flush { reply: flushed_tx }).is_ok() {
        let _ = flushed_rx.await;
    }
    fetch_company_sites(&db, source.as_ref(), &ScanLimits::default()).await;
}

const MAX_COMPANIES_PER_RUN: usize = 8;

async fn fetch_company_sites<S: JobSource>(db: &Db, source: &S, limits: &ScanLimits) {
    let db_for_list = db.clone();
    let urls = match tokio::task::spawn_blocking(move || {
        db_for_list.companies_missing_site(MAX_COMPANIES_PER_RUN)
    })
    .await
    {
        Ok(Ok(urls)) => urls,
        Ok(Err(e)) => {
            tracing::error!("couldn't list companies: {e}");
            return;
        }
        Err(e) => {
            tracing::error!("couldn't list companies: {e}");
            return;
        }
    };
    for url in urls {
        pause(limits).await;
        let website = match source.company_website(&url).await {
            Ok(website) => website,
            Err(LinkedInError::Blocked(status)) => {
                tracing::warn!(status, "company pages refused, trying again next scan");
                return;
            }
            Err(LinkedInError::Status(status)) => {
                tracing::warn!(status, url, "company page unavailable");
                None
            }
            Err(e) => {
                tracing::warn!(url, "company page failed: {e}");
                continue;
            }
        };
        let db = db.clone();
        match tokio::task::spawn_blocking(move || db.save_company_site(&url, website.as_deref()))
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!("couldn't save company site: {e}"),
            Err(e) => tracing::error!("couldn't save company site: {e}"),
        }
    }
}

async fn scrape_searches<S: JobSource>(
    db: &Db,
    source: &S,
    limits: &ScanLimits,
    mut on_batch: impl FnMut(&JobSearch, Vec<ScrapedJob>),
) -> ScanReport {
    let db_for_searches = db.clone();
    let searches = match tokio::task::spawn_blocking(move || db_for_searches.list_searches()).await
    {
        Ok(Ok(searches)) => searches,
        Ok(Err(e)) => {
            return ScanReport {
                error: Some(e.to_string()),
                ..ScanReport::default()
            };
        }
        Err(e) => {
            return ScanReport {
                error: Some(e.to_string()),
                ..ScanReport::default()
            };
        }
    };

    let mut report = ScanReport::default();
    let mut budget = Budget {
        load_more: limits.max_load_more_per_run,
        details: limits.max_details_per_run,
    };
    let mut fetched_descriptions = HashMap::new();
    for search in searches.iter().filter(|search| search.enabled) {
        let (jobs, result) = scrape_search(
            db,
            source,
            limits,
            search,
            &mut budget,
            &mut report,
            &mut fetched_descriptions,
        )
        .await;
        on_batch(search, jobs);
        if let Err(error) = result {
            report.error = Some(error.to_string());
        }
        if report.search_blocked {
            break;
        }
    }
    report
}

async fn scrape_search<S: JobSource>(
    db: &Db,
    source: &S,
    limits: &ScanLimits,
    search: &JobSearch,
    budget: &mut Budget,
    report: &mut ScanReport,
    fetched_descriptions: &mut HashMap<String, String>,
) -> (Vec<ScrapedJob>, Result<(), ScanError>) {
    let criteria = &search.criteria;
    let queries = if criteria.titles.is_empty() {
        vec![criteria.keywords.join(" ")]
    } else {
        criteria.titles.clone()
    };
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
                Err(LinkedInError::Blocked(_)) if !first_page => {
                    budget.load_more = 0;
                    break;
                }
                Err(LinkedInError::Status(_)) if !first_page => break,
                Err(error) => {
                    report.search_blocked |= matches!(error, LinkedInError::Blocked(_));
                    report.error = Some(error.to_string());
                    break 'queries;
                }
            };
            if cards.is_empty() {
                break;
            }
            start += cards.len();
            for card in cards {
                let wanted =
                    criteria.titles.is_empty() || title_matches(&card.title, &criteria.titles);
                if wanted && seen.insert(card.linkedin_id.clone()) {
                    candidates.push(card);
                }
            }
        }
    }

    let mut jobs = Vec::new();
    for card in candidates {
        let mut description = if let Some(description) = fetched_descriptions.get(&card.linkedin_id)
        {
            Some(description.clone())
        } else {
            let db_for_known = db.clone();
            let linkedin_id = card.linkedin_id.clone();
            match tokio::task::spawn_blocking(move || db_for_known.known_job(&linkedin_id)).await {
                Ok(Ok(known)) => known
                    .map(|job| {
                        let _id = job.id;
                        job.description
                    })
                    .flatten(),
                Ok(Err(error)) => return (jobs, Err(error.into())),
                Err(error) => {
                    return (
                        jobs,
                        Err(DbError::Io(std::io::Error::other(error.to_string())).into()),
                    )
                }
            }
        };
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
                        description = detail.as_ref().map(|job| job.description.clone());
                        if let Some(description) = &description {
                            fetched_descriptions
                                .insert(card.linkedin_id.clone(), description.clone());
                        }
                    }
                    Err(LinkedInError::Blocked(_)) => {
                        budget.details = 0;
                        report.details_paused = true;
                        report.pending_details += 1;
                    }
                    Err(error) => return (jobs, Err(error.into())),
                }
            }
        }
        jobs.push(ScrapedJob {
            card,
            detail,
            description,
        });
    }
    (jobs, Ok(()))
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
    let ms = rand::thread_rng()
        .gen_range(limits.min_delay.as_millis() as u64..=limits.max_delay.as_millis() as u64);
    Duration::from_millis(ms)
}

#[cfg(test)]
pub(crate) async fn run_scan_for_test<S: JobSource>(
    db: &Db,
    source: &S,
    limits: &ScanLimits,
) -> ScanReport {
    let mut new_matches = 0;
    let mut match_error = None;
    let mut report =
        scrape_searches(
            db,
            source,
            limits,
            |search, jobs| match crate::match_stage::apply_batch(db, search, jobs) {
                Ok(count) => new_matches += count,
                Err(error) => match_error = Some(error.to_string()),
            },
        )
        .await;
    report.new_matches = new_matches;
    if let Some(error) = match_error {
        report.error = Some(error);
    }
    report
}

const SEARCH_PAGE_URL: &str = "https://www.linkedin.com/jobs/search";
const SEE_MORE_URL: &str = "https://www.linkedin.com/jobs-guest/jobs/api/seeMoreJobPostings/search";
const DETAIL_URL: &str = "https://www.linkedin.com/jobs/view";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

pub struct SearchQuery<'a> {
    pub keywords: &'a str,
    pub location: Option<&'a str>,
    pub posted_within: PostedWithin,
    pub work_modes: &'a [WorkMode],
    pub start: usize,
}

pub struct LinkedInClient {
    http: reqwest::Client,
}

impl LinkedInClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,*/*;q=0.8".parse().unwrap(),
        );
        headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            "en-US,en;q=0.9".parse().unwrap(),
        );
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .cookie_store(true)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http })
    }

    pub async fn search(&self, query: &SearchQuery<'_>) -> Result<Vec<JobCard>, LinkedInError> {
        let mut params = vec![
            ("keywords", query.keywords.to_string()),
            ("f_TPR", query.posted_within.linkedin_param().to_string()),
            ("sortBy", "DD".to_string()),
        ];
        if let Some(location) = query.location {
            params.push(("location", location.to_string()));
        }
        if !query.work_modes.is_empty() {
            let codes: Vec<&str> = query
                .work_modes
                .iter()
                .map(|m| match m {
                    WorkMode::Onsite => "1",
                    WorkMode::Remote => "2",
                    WorkMode::Hybrid => "3",
                })
                .collect();
            params.push(("f_WT", codes.join(",")));
        }
        let html = if query.start == 0 {
            self.get("search", SEARCH_PAGE_URL, &params, None).await?
        } else {
            let referer = reqwest::Url::parse_with_params(SEARCH_PAGE_URL, &params).ok();
            params.push(("start", query.start.to_string()));
            self.get(
                "search-more",
                SEE_MORE_URL,
                &params,
                referer.as_ref().map(reqwest::Url::as_str),
            )
            .await?
        };
        Ok(parse_search_results(&html))
    }

    pub async fn job_detail(&self, linkedin_id: &str) -> Result<Option<JobDetail>, LinkedInError> {
        match self
            .get("detail", &format!("{DETAIL_URL}/{linkedin_id}"), &[], None)
            .await
        {
            Ok(html) => Ok(parse_job_detail(&html)),
            Err(LinkedInError::Status(404 | 410)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn company_website(&self, company_url: &str) -> Result<Option<String>, LinkedInError> {
        let html = self.get("company", company_url, &[], None).await?;
        Ok(parse_company_website(&html))
    }

    async fn get(
        &self,
        kind: &str,
        url: &str,
        params: &[(&str, String)],
        referer: Option<&str>,
    ) -> Result<String, LinkedInError> {
        let mut request = self.http.get(url).query(params);
        if let Some(referer) = referer {
            request = request.header(reqwest::header::REFERER, referer);
        }
        let res = request.send().await?;
        let status = res.status().as_u16();
        let start = params
            .iter()
            .find(|(k, _)| *k == "start")
            .map(|(_, v)| v.as_str())
            .unwrap_or("-");
        tracing::info!(kind, status, start, "linkedin request");
        match status {
            200..=299 => Ok(res.text().await?),
            429 | 999 => Err(LinkedInError::Blocked(status)),
            _ => Err(LinkedInError::Status(status)),
        }
    }
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static selector is valid")
}

fn clean_text(el: ElementRef) -> String {
    el.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn without_query(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).to_string()
}

pub fn parse_search_results(html: &str) -> Vec<JobCard> {
    let doc = Html::parse_document(html);
    let card_sel = selector(r#"[data-entity-urn^="urn:li:jobPosting:"]"#);
    let mut cards: Vec<JobCard> = Vec::new();
    for card in doc.select(&card_sel) {
        if let Some(parsed) = parse_card(card) {
            if !cards.iter().any(|c| c.linkedin_id == parsed.linkedin_id) {
                cards.push(parsed);
            }
        }
    }
    cards
}

fn parse_card(card: ElementRef) -> Option<JobCard> {
    let urn = card.value().attr("data-entity-urn")?;
    let linkedin_id = urn.rsplit(':').next()?.to_string();
    if linkedin_id.is_empty() || !linkedin_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let first = |css: &str| card.select(&selector(css)).next();
    let title = first(".base-search-card__title")
        .map(clean_text)
        .filter(|t| !t.is_empty())?;
    let company = first(".base-search-card__subtitle")
        .map(clean_text)
        .unwrap_or_default();
    let company_url = first(".base-search-card__subtitle a[href]")
        .and_then(|a| a.value().attr("href"))
        .map(without_query);
    let location = first(".job-search-card__location")
        .map(clean_text)
        .unwrap_or_default();
    let posted_on = first("time[datetime]")
        .and_then(|t| t.value().attr("datetime"))
        .map(str::to_string);
    let url = first("a.base-card__full-link[href]")
        .and_then(|a| a.value().attr("href"))
        .or_else(|| card.value().attr("href"))
        .map(without_query)
        .unwrap_or_else(|| format!("https://www.linkedin.com/jobs/view/{linkedin_id}"));

    Some(JobCard {
        linkedin_id,
        title,
        company,
        company_url,
        location,
        posted_on,
        url,
    })
}

pub fn parse_company_website(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let href = doc
        .select(&selector(r#"[data-test-id="about-us__website"] a[href]"#))
        .next()?
        .value()
        .attr("href")?;
    let mut url = reqwest::Url::parse(href.trim()).ok()?;
    if url.path().starts_with("/redir/redirect") {
        let target = url
            .query_pairs()
            .find(|(key, _)| key == "url")
            .map(|(_, value)| value.into_owned())?;
        url = reqwest::Url::parse(target.trim()).ok()?;
    }
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

pub fn parse_job_detail(html: &str) -> Option<JobDetail> {
    let doc = Html::parse_document(html);
    let markup = doc
        .select(&selector(".show-more-less-html__markup"))
        .next()?;
    let description = block_text(markup);
    if description.is_empty() {
        return None;
    }

    let mut detail = JobDetail {
        description,
        seniority: None,
        employment_type: None,
        industries: None,
    };
    let header_sel = selector(".description__job-criteria-subheader");
    let value_sel = selector(".description__job-criteria-text");
    for item in doc.select(&selector(".description__job-criteria-item")) {
        let (Some(header), Some(value)) = (
            item.select(&header_sel).next(),
            item.select(&value_sel).next(),
        ) else {
            continue;
        };
        let value = Some(clean_text(value)).filter(|v| !v.is_empty());
        match clean_text(header).to_lowercase().as_str() {
            "seniority level" => detail.seniority = value,
            "employment type" => detail.employment_type = value,
            "industries" => detail.industries = value,
            _ => {}
        }
    }
    Some(detail)
}

fn block_text(root: ElementRef) -> String {
    fn walk(el: ElementRef, out: &mut String) {
        for child in el.children() {
            if let Some(text) = child.value().as_text() {
                let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !collapsed.is_empty() {
                    if text.starts_with(char::is_whitespace) {
                        out.push(' ');
                    }
                    out.push_str(&collapsed);
                    if text.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                }
            } else if let Some(child_el) = ElementRef::wrap(child) {
                let name = child_el.value().name();
                match name {
                    "br" => out.push('\n'),
                    "li" => {
                        out.push_str("\n- ");
                        walk(child_el, out);
                    }
                    "p" | "div" | "ul" | "ol" | "section" | "h1" | "h2" | "h3" | "h4" | "h5"
                    | "h6" => {
                        out.push('\n');
                        walk(child_el, out);
                        out.push('\n');
                    }
                    _ => walk(child_el, out),
                }
            }
        }
    }

    let mut raw = String::new();
    walk(root, &mut raw);

    let mut out = String::new();
    let mut blank_run = 0;
    for line in raw.lines().map(str::trim) {
        if line.is_empty() {
            blank_run += 1;
            if blank_run == 1 && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        blank_run = 0;
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;
    use crate::types::{PostedWithin, SearchInput, WorkMode};

    #[derive(Default)]
    struct FakeSource {
        pages: HashMap<String, Vec<Vec<JobCard>>>,
        details: HashMap<String, String>,
        block_details: bool,
        block_after_first_page: bool,
        blocked_queries: Vec<String>,
        detail_calls: Mutex<usize>,
    }

    impl JobSource for FakeSource {
        fn search(
            &self,
            query: &SearchQuery<'_>,
        ) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send {
            let pages = self.pages.get(query.keywords).cloned().unwrap_or_default();
            let blocked = self.blocked_queries.iter().any(|q| q == query.keywords);
            let result = if blocked || (self.block_after_first_page && query.start > 0) {
                Err(LinkedInError::Blocked(429))
            } else {
                Ok(pages.get(query.start / 2).cloned().unwrap_or_default())
            };
            async move { result }
        }

        fn job_detail(
            &self,
            id: &str,
        ) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send {
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
        ScanLimits {
            max_load_more_per_run: 2,
            max_details_per_run: 10,
            min_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        }
    }

    fn setup(modes: Vec<WorkMode>) -> (Db, std::path::PathBuf) {
        setup_with(vec!["Backend Engineer".into()], modes)
    }

    fn setup_with(titles: Vec<String>, modes: Vec<WorkMode>) -> (Db, std::path::PathBuf) {
        let dir = crate::db::test_dir("tailor-scan-test");
        let store = Db::open(&dir).unwrap();
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
                vec![
                    card("1", "Senior Backend Engineer"),
                    card("2", "iOS Developer"),
                ],
                vec![
                    card("3", "Backend Engineer (Remote)"),
                    card("4", "Backend Engineer, Payments"),
                ],
            ],
        );
        source
            .details
            .insert("1".into(), "Hybrid role. We use Rust.".into());
        source.details.insert("3".into(), "Rust everywhere.".into());
        source.details.insert("4".into(), "Java shop.".into());
        source
    }

    #[tokio::test]
    async fn filters_by_title_keywords_and_work_mode() {
        let (store, dir) = setup(vec![WorkMode::Hybrid, WorkMode::Onsite]);
        let src = source();
        let report = run_scan_for_test(&store, &src, &fast()).await;

        assert_eq!(report.error, None);
        assert_eq!(*src.detail_calls.lock().unwrap(), 3);
        let jobs = store.list_jobs(None).unwrap();
        assert_eq!(
            jobs.iter()
                .map(|j| j.linkedin_id.as_str())
                .collect::<Vec<_>>(),
            vec!["1"]
        );
        assert_eq!(jobs[0].matched_keywords, vec!["Rust"]);
        assert_eq!(jobs[0].work_mode, Some(WorkMode::Hybrid));
        assert_eq!(report.new_matches, 1);

        let again = run_scan_for_test(&store, &src, &fast()).await;
        assert_eq!(*src.detail_calls.lock().unwrap(), 3);
        assert_eq!(again.new_matches, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn detail_budget_defers_to_next_scan() {
        let (store, dir) = setup(vec![WorkMode::Remote, WorkMode::Hybrid]);
        let src = source();
        let limits = ScanLimits {
            max_details_per_run: 1,
            ..fast()
        };

        let first = run_scan_for_test(&store, &src, &limits).await;
        assert_eq!((first.new_matches, first.pending_details), (1, 2));
        let second = run_scan_for_test(&store, &src, &limits).await;
        assert_eq!((second.new_matches, second.pending_details), (1, 1));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn paging_cap_keeps_first_page() {
        let (store, dir) = setup(vec![WorkMode::Hybrid, WorkMode::Onsite]);
        let src = FakeSource {
            block_after_first_page: true,
            ..source()
        };
        let report = run_scan_for_test(&store, &src, &fast()).await;
        assert_eq!(report.error, None);
        assert_eq!(report.new_matches, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn blocked_results_page_keeps_collected_jobs() {
        let titles = vec!["Backend Engineer".into(), "Platform Engineer".into()];
        let (store, dir) = setup_with(titles, vec![WorkMode::Hybrid, WorkMode::Onsite]);
        let src = FakeSource {
            blocked_queries: vec!["Platform Engineer".into()],
            ..source()
        };
        let report = run_scan_for_test(&store, &src, &fast()).await;
        assert!(report.search_blocked);
        assert!(report.error.unwrap().contains("rate-limiting"));
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
        let limits = ScanLimits {
            max_load_more_per_run: 1,
            ..fast()
        };
        let report = run_scan_for_test(&store, &src, &limits).await;
        let searches = report.requests - *src.detail_calls.lock().unwrap();
        assert_eq!(searches, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn without_keywords_jobs_list_before_descriptions() {
        let (store, dir) = setup_with(
            vec!["Backend Engineer".into()],
            vec![WorkMode::Hybrid, WorkMode::Onsite],
        );
        store
            .update_search(store.list_searches().unwrap()[0].id, &{
                let mut c = store.list_searches().unwrap()[0].criteria.clone();
                c.keywords.clear();
                c
            })
            .unwrap();
        let src = source();
        let limits = ScanLimits {
            max_details_per_run: 0,
            ..fast()
        };
        let report = run_scan_for_test(&store, &src, &limits).await;
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
        src.details
            .insert("4".into(), "Fully on-site in Dubai.".into());
        run_scan_for_test(
            &store,
            &src,
            &ScanLimits {
                max_details_per_run: 0,
                ..fast()
            },
        )
        .await;
        assert_eq!(store.list_jobs(None).unwrap().len(), 3);
        run_scan_for_test(&store, &src, &fast()).await;
        let ids: Vec<String> = store
            .list_jobs(None)
            .unwrap()
            .into_iter()
            .map(|j| j.linkedin_id)
            .collect();
        assert_eq!(ids, vec!["3"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn blocked_job_pages_pause_softly() {
        let (store, dir) = setup(vec![WorkMode::Hybrid]);
        let src = FakeSource {
            block_details: true,
            ..source()
        };
        let report = run_scan_for_test(&store, &src, &fast()).await;
        assert_eq!(report.error, None);
        assert!(report.details_paused);
        assert_eq!(*src.detail_calls.lock().unwrap(), 1);
        assert_eq!(report.pending_details, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn blocked_search_stops_the_scan() {
        let (store, dir) = setup(vec![WorkMode::Hybrid]);

        struct AlwaysBlocked;
        impl JobSource for AlwaysBlocked {
            fn search(
                &self,
                _: &SearchQuery<'_>,
            ) -> impl Future<Output = Result<Vec<JobCard>, LinkedInError>> + Send {
                async { Err(LinkedInError::Blocked(429)) }
            }
            fn job_detail(
                &self,
                _: &str,
            ) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send {
                async { Ok(None) }
            }
        }
        let report = run_scan_for_test(&store, &AlwaysBlocked, &fast()).await;
        assert!(report.error.unwrap().contains("rate-limiting"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod linkedin_tests {
    use super::*;

    const SEARCH_FIXTURE: &str = r#"
      <li><div class="base-card job-search-card" data-entity-urn="urn:li:jobPosting:4362132219">
        <a class="base-card__full-link" href="https://ae.linkedin.com/jobs/view/senior-software-engineer-backend-at-talabat-4362132219?position=1&amp;refId=x">
          <span class="sr-only">Senior Software Engineer- Backend</span></a>
        <div class="base-search-card__info">
          <h3 class="base-search-card__title">
            Senior Software Engineer- Backend
          </h3>
          <h4 class="base-search-card__subtitle">
            <a class="hidden-nested-link" href="https://ae.linkedin.com/company/talabat-com?trk=public_jobs">talabat</a>
          </h4>
          <div class="base-search-card__metadata">
            <span class="job-search-card__location">
              Dubai, Dubai, United Arab Emirates
            </span>
            <time class="job-search-card__listdate" datetime="2026-10-13">6 days ago</time>
          </div>
        </div>
      </div></li>
      <li><div class="base-card job-search-card" data-entity-urn="urn:li:jobPosting:4480501557">
        <a class="base-card__full-link" href="https://ae.linkedin.com/jobs/view/backend-architect-4480501557?position=2"></a>
        <h3 class="base-search-card__title">Backend Architect - Pay&amp;Card Tech</h3>
        <h4 class="base-search-card__subtitle"><a href="https://www.linkedin.com/company/binance?trk=x">Binance</a></h4>
        <span class="job-search-card__location">United Arab Emirates</span>
        <time class="job-search-card__listdate--new" datetime="2026-10-16">3 days ago</time>
      </div></li>
      <li><div class="base-card" data-entity-urn="urn:li:jobPosting:not-a-number"></div></li>
    "#;

    const DETAIL_FIXTURE: &str = r#"
      <section class="description">
        <div class="show-more-less-html__markup show-more-less-html__markup--clamp-after-5">
          <strong>About us</strong><br><br>We build things.<br>
          <ul><li>Rust &amp; Kafka</li><li>Hybrid, 3 days in office</li></ul>
          <p>Apply now.</p>
        </div>
      </section>
      <ul class="description__job-criteria-list">
        <li class="description__job-criteria-item">
          <h3 class="description__job-criteria-subheader">Seniority level</h3>
          <span class="description__job-criteria-text description__job-criteria-text--criteria">Associate</span>
        </li>
        <li class="description__job-criteria-item">
          <h3 class="description__job-criteria-subheader">Employment type</h3>
          <span class="description__job-criteria-text">Full-time</span>
        </li>
      </ul>
    "#;

    #[test]
    fn parses_search_cards() {
        let cards = parse_search_results(SEARCH_FIXTURE);
        assert_eq!(cards.len(), 2);
        assert_eq!(
            cards[0],
            JobCard {
                linkedin_id: "4362132219".into(),
                title: "Senior Software Engineer- Backend".into(),
                company: "talabat".into(),
                company_url: Some("https://ae.linkedin.com/company/talabat-com".into()),
                location: "Dubai, Dubai, United Arab Emirates".into(),
                posted_on: Some("2026-10-13".into()),
                url: "https://ae.linkedin.com/jobs/view/senior-software-engineer-backend-at-talabat-4362132219".into(),
            }
        );
        assert_eq!(cards[1].title, "Backend Architect - Pay&Card Tech");
        assert_eq!(cards[1].posted_on.as_deref(), Some("2026-10-16"));
    }

    #[test]
    fn parses_job_detail() {
        let detail = parse_job_detail(DETAIL_FIXTURE).unwrap();
        assert_eq!(detail.description, "About us\n\nWe build things.\n\n- Rust & Kafka\n- Hybrid, 3 days in office\n\nApply now.");
        assert_eq!(detail.seniority.as_deref(), Some("Associate"));
        assert_eq!(detail.employment_type.as_deref(), Some("Full-time"));
        assert_eq!(detail.industries, None);
    }

    #[test]
    fn detail_without_description_is_none() {
        assert!(parse_job_detail("<html><body>Sign in</body></html>").is_none());
    }

    #[test]
    fn company_website_is_unwrapped_from_redirect() {
        let html = r#"<dl><div data-test-id="about-us__website"><dt>Website</dt><dd>
            <a class="link-no-visited-state" href="https://www.linkedin.com/redir/redirect?url=https%3A%2F%2Fcmhs%2Euaeu%2Eac%2Eae%2Fen%2Fdepartments%2Ffmd%2F&amp;urlhash=3-Nx&amp;trk=about_website">cmhs.uaeu.ac.ae</a>
        </dd></div></dl>"#;
        assert_eq!(
            parse_company_website(html).as_deref(),
            Some("https://cmhs.uaeu.ac.ae/en/departments/fmd/")
        );
    }

    #[test]
    fn company_website_rejects_missing_or_unsafe_links() {
        assert!(parse_company_website("<html><body>Sign in</body></html>").is_none());
        let html = r#"<div data-test-id="about-us__website"><a href="javascript:alert(1)">x</a></div>"#;
        assert!(parse_company_website(html).is_none());
    }
}
