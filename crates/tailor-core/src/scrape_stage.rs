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
    errors::{DbError, IndeedError, LinkedInError, ScanError},
    match_stage::{title_matches, MatchMessage},
    types::{JobCard, JobDetail, JobSearch, PostedWithin, ScanStatus, Site, WorkMode},
};

#[derive(Debug, Clone)]
pub struct ScrapedJob {
    pub card: JobCard,
    pub detail: Option<JobDetail>,
    pub description: Option<String>,
    pub work_mode: Option<WorkMode>,
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
    pub indeed_error: Option<String>,
    pub indeed_unknown_locations: Vec<String>,
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
        source_id: &str,
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
        source_id: &str,
    ) -> impl Future<Output = Result<Option<JobDetail>, LinkedInError>> + Send {
        LinkedInClient::job_detail(self, source_id)
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
        indeed: IndeedClient,
        cooldown_seconds: i64,
        rx: UnboundedReceiver<ScrapeMessage>,
        matcher: UnboundedSender<MatchMessage>,
    ) -> Self {
        let handle = tokio::spawn(run(
            db,
            Arc::new(client),
            Arc::new(indeed),
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
    indeed: Arc<IndeedClient>,
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
                let (db, client, indeed, matcher, done_tx) = (
                    db.clone(),
                    client.clone(),
                    indeed.clone(),
                    matcher.clone(),
                    done_tx.clone(),
                );
                tokio::spawn(async move {
                    scrape_run(db, client, indeed, matcher).await;
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

async fn scrape_run<S: JobSource + Send + Sync + 'static, I: IndeedSource + Send + Sync + 'static>(
    db: Arc<Db>,
    source: Arc<S>,
    indeed: Arc<I>,
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
    let send_batch = |search: &JobSearch, jobs| {
        let _ = matcher.send(MatchMessage::Batch {
            search: search.clone(),
            jobs,
        });
    };
    let mut report =
        scrape_searches(&db, source.as_ref(), &ScanLimits::default(), send_batch).await;
    let indeed_report =
        scrape_indeed(&db, indeed.as_ref(), &ScanLimits::default(), send_batch).await;
    report.requests += indeed_report.requests;
    report.indeed_error = indeed_report.error;
    report.indeed_unknown_locations = indeed_report.unknown_locations;
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
                if wanted && seen.insert(card.source_id.clone()) {
                    candidates.push(card);
                }
            }
        }
    }

    let mut jobs = Vec::new();
    for card in candidates {
        let mut description = if let Some(description) = fetched_descriptions.get(&card.source_id)
        {
            Some(description.clone())
        } else {
            let db_for_known = db.clone();
            let source_id = card.source_id.clone();
            match tokio::task::spawn_blocking(move || db_for_known.known_job(&source_id)).await {
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
                match source.job_detail(&card.source_id).await {
                    Ok(fetched) => {
                        detail = fetched;
                        description = detail.as_ref().map(|job| job.description.clone());
                        if let Some(description) = &description {
                            fetched_descriptions
                                .insert(card.source_id.clone(), description.clone());
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
            work_mode: None,
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

const MAX_INDEED_REQUESTS_PER_RUN: usize = 12;
const MAX_INDEED_PAGES_PER_QUERY: usize = 2;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct IndeedReport {
    pub requests: usize,
    pub error: Option<String>,
    pub unknown_locations: Vec<String>,
}

async fn scrape_indeed<I: IndeedSource>(
    db: &Db,
    source: &I,
    limits: &ScanLimits,
    mut on_batch: impl FnMut(&JobSearch, Vec<ScrapedJob>),
) -> IndeedReport {
    let mut report = IndeedReport::default();
    let db_for_searches = db.clone();
    let searches = match tokio::task::spawn_blocking(move || db_for_searches.list_searches()).await
    {
        Ok(Ok(searches)) => searches,
        Ok(Err(e)) => {
            report.error = Some(e.to_string());
            return report;
        }
        Err(e) => {
            report.error = Some(e.to_string());
            return report;
        }
    };

    let mut budget = MAX_INDEED_REQUESTS_PER_RUN;
    for search in searches.iter().filter(|search| search.enabled) {
        let criteria = &search.criteria;
        let location = criteria.location.as_deref();
        let Some(country) = indeed_country(location) else {
            report
                .unknown_locations
                .push(location.unwrap_or("no location").to_string());
            continue;
        };
        let queries = if criteria.titles.is_empty() {
            vec![criteria.keywords.join(" ")]
        } else {
            criteria.titles.clone()
        };
        let remote_only = criteria.work_modes == [WorkMode::Remote];
        let mut seen = HashSet::new();
        let mut found: Vec<IndeedJob> = Vec::new();
        let mut blocked = false;
        'queries: for keywords in &queries {
            let mut cursor: Option<String> = None;
            for _ in 0..MAX_INDEED_PAGES_PER_QUERY {
                if budget == 0 {
                    break 'queries;
                }
                budget -= 1;
                pause(limits).await;
                report.requests += 1;
                let query = IndeedQuery {
                    keywords,
                    location,
                    country,
                    posted_within: criteria.posted_within,
                    remote_only,
                    cursor: cursor.as_deref(),
                };
                let page = match source.search(&query).await {
                    Ok(page) => page,
                    Err(error) => {
                        blocked = matches!(error, IndeedError::Blocked(_));
                        report.error = Some(error.to_string());
                        break 'queries;
                    }
                };
                for job in page.jobs {
                    let wanted = criteria.titles.is_empty()
                        || title_matches(&job.card.title, &criteria.titles);
                    if wanted && seen.insert(job.card.source_id.clone()) {
                        found.push(job);
                    }
                }
                match page.next_cursor {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
        }

        let sites: Vec<(String, String)> = found
            .iter()
            .filter_map(|job| Some((job.card.company_url.clone()?, job.company_website.clone()?)))
            .collect();
        if !sites.is_empty() {
            let db = db.clone();
            let saved = tokio::task::spawn_blocking(move || {
                sites
                    .iter()
                    .try_for_each(|(url, website)| db.save_company_site(url, Some(website)))
            })
            .await;
            match saved {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!("couldn't save company site: {e}"),
                Err(e) => tracing::error!("couldn't save company site: {e}"),
            }
        }

        let jobs = found
            .into_iter()
            .map(|job| ScrapedJob {
                description: Some(job.detail.description.clone()),
                detail: Some(job.detail),
                card: job.card,
                work_mode: job.work_mode,
            })
            .collect();
        on_batch(search, jobs);
        if blocked {
            break;
        }
    }
    report
}

const INDEED_API_URL: &str = "https://apis.indeed.com/graphql";
const INDEED_API_KEY: &str = "161092c2017b5bbab13edb12461a62d5a833871e7cad6d9d475304573de67ac8";
const INDEED_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 Indeed App 193.1";
const INDEED_APP_INFO: &str = "appv=193.1; appid=com.indeed.jobsearch; osv=16.6.1; os=ios; dtype=phone";
const INDEED_PAGE_SIZE: usize = 100;
const INDEED_REMOTE_KEY: &str = "DSQF7";
const INDEED_HYBRID_KEY: &str = "PAXZC";
const INDEED_JOB_TYPE_KEYS: &[&str] = &[
    "CF3CP", "75GKK", "NJXCK", "T9BXE", "T65DZ", "VDTG7", "4HKF7", "CJWTS", "5QWDV",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndeedCountry {
    pub code: &'static str,
    pub host: &'static str,
}

const INDEED_COUNTRIES: &[(&str, &str, &str)] = &[
    ("AE", "ae.indeed.com", "united arab emirates|uae|dubai|abu dhabi|sharjah|ajman|ras al khaimah|fujairah|al ain"),
    ("SA", "sa.indeed.com", "saudi arabia|ksa|riyadh|jeddah|dammam|khobar"),
    ("QA", "qa.indeed.com", "qatar|doha"),
    ("BH", "bh.indeed.com", "bahrain|manama"),
    ("KW", "kw.indeed.com", "kuwait"),
    ("OM", "om.indeed.com", "oman|muscat"),
    ("EG", "eg.indeed.com", "egypt|cairo|alexandria"),
    ("US", "www.indeed.com", "united states|united states of america|usa|us|new york|san francisco|seattle|austin|boston|chicago|los angeles|new york city|nyc|california|texas|washington"),
    ("GB", "uk.indeed.com", "united kingdom|uk|great britain|england|scotland|wales|london|manchester|edinburgh|birmingham|bristol|cambridge|oxford"),
    ("IE", "ie.indeed.com", "ireland|dublin|cork"),
    ("CA", "ca.indeed.com", "canada|toronto|vancouver|montreal|ottawa|calgary"),
    ("DE", "de.indeed.com", "germany|deutschland|berlin|munich|münchen|hamburg|frankfurt|cologne|köln"),
    ("FR", "fr.indeed.com", "france|paris|lyon"),
    ("NL", "nl.indeed.com", "netherlands|the netherlands|amsterdam|rotterdam|utrecht|eindhoven"),
    ("ES", "es.indeed.com", "spain|madrid|barcelona"),
    ("PT", "pt.indeed.com", "portugal|lisbon|lisboa|porto"),
    ("IT", "it.indeed.com", "italy|milan|rome"),
    ("CH", "ch.indeed.com", "switzerland|zurich|zürich|geneva"),
    ("SE", "se.indeed.com", "sweden|stockholm"),
    ("PL", "pl.indeed.com", "poland|warsaw|krakow|kraków"),
    ("IN", "in.indeed.com", "india|bangalore|bengaluru|mumbai|delhi|new delhi|hyderabad|pune|chennai|gurgaon|gurugram|noida"),
    ("PK", "pk.indeed.com", "pakistan|karachi|lahore|islamabad"),
    ("SG", "sg.indeed.com", "singapore"),
    ("MY", "malaysia.indeed.com", "malaysia|kuala lumpur"),
    ("AU", "au.indeed.com", "australia|sydney|melbourne|brisbane|perth"),
    ("NZ", "nz.indeed.com", "new zealand|auckland|wellington"),
    ("ZA", "za.indeed.com", "south africa|cape town|johannesburg"),
    ("NG", "ng.indeed.com", "nigeria|lagos"),
    ("JP", "jp.indeed.com", "japan|tokyo"),
];

pub fn indeed_country(location: Option<&str>) -> Option<IndeedCountry> {
    let location = location?.to_lowercase();
    location
        .rsplit(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .find_map(|part| {
            INDEED_COUNTRIES
                .iter()
                .find(|(_, _, names)| names.split('|').any(|name| name == part))
        })
        .map(|(code, host, _)| IndeedCountry { code, host })
}

pub struct IndeedQuery<'a> {
    pub keywords: &'a str,
    pub location: Option<&'a str>,
    pub country: IndeedCountry,
    pub posted_within: PostedWithin,
    pub remote_only: bool,
    pub cursor: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndeedJob {
    pub card: JobCard,
    pub detail: JobDetail,
    pub work_mode: Option<WorkMode>,
    pub company_website: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndeedPage {
    pub jobs: Vec<IndeedJob>,
    pub next_cursor: Option<String>,
}

pub trait IndeedSource {
    fn search(
        &self,
        query: &IndeedQuery<'_>,
    ) -> impl Future<Output = Result<IndeedPage, IndeedError>> + Send;
}

pub struct IndeedClient {
    http: reqwest::Client,
}

impl IndeedClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("indeed-api-key", reqwest::header::HeaderValue::from_static(INDEED_API_KEY));
        headers.insert("indeed-app-info", reqwest::header::HeaderValue::from_static(INDEED_APP_INFO));
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            reqwest::header::HeaderValue::from_static("en-US,en;q=0.9"),
        );
        let http = reqwest::Client::builder()
            .user_agent(INDEED_USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http })
    }
}

impl IndeedSource for IndeedClient {
    async fn search(&self, query: &IndeedQuery<'_>) -> Result<IndeedPage, IndeedError> {
        let res = self
            .http
            .post(INDEED_API_URL)
            .header("indeed-co", query.country.code)
            .json(&serde_json::json!({ "query": indeed_graphql(query) }))
            .send()
            .await?;
        let status = res.status().as_u16();
        tracing::info!(kind = "indeed", status, "indeed request");
        match status {
            200..=299 => {}
            403 | 429 => return Err(IndeedError::Blocked(status)),
            _ => return Err(IndeedError::Status(status)),
        }
        let body: serde_json::Value = res.json().await?;
        parse_indeed_page(&body, query.country.host)
    }
}

fn indeed_graphql(query: &IndeedQuery<'_>) -> String {
    let quote = |text: &str| serde_json::Value::from(text).to_string();
    let hours = match query.posted_within {
        PostedWithin::Day => 24,
        PostedWithin::Week => 24 * 7,
        PostedWithin::Month => 24 * 30,
    };
    let mut filters =
        vec![format!(r#"{{ date: {{ field: "dateOnIndeed", start: "{hours}h" }} }}"#)];
    if query.remote_only {
        filters.push(format!(
            r#"{{ keyword: {{ field: "attributes", keys: ["{INDEED_REMOTE_KEY}"] }} }}"#
        ));
    }
    let location = query
        .location
        .map(|place| format!("location: {{ where: {}, radius: 50, radiusUnit: MILES }}", quote(place)))
        .unwrap_or_default();
    let cursor = query
        .cursor
        .map(|cursor| format!("cursor: {}", quote(cursor)))
        .unwrap_or_default();
    format!(
        r#"query GetJobData {{
  jobSearch(what: {what} {location} limit: {INDEED_PAGE_SIZE} {cursor} sort: RELEVANCE filters: [{filters}]) {{
    pageInfo {{ nextCursor }}
    results {{
      job {{
        key
        title
        sourceEmployerName
        dateOnIndeed
        description {{ html }}
        location {{ city countryName formatted {{ long }} }}
        attributes {{ key label }}
        employer {{
          name
          relativeCompanyPageUrl
          dossier {{ employerDetails {{ industry }} links {{ corporateWebsite }} }}
        }}
      }}
    }}
  }}
}}"#,
        what = quote(query.keywords),
        filters = filters.join(", "),
    )
}

pub fn parse_indeed_page(body: &serde_json::Value, host: &str) -> Result<IndeedPage, IndeedError> {
    if let Some(message) = body["errors"][0]["message"].as_str() {
        return Err(IndeedError::BadResponse(message.chars().take(200).collect()));
    }
    let search = &body["data"]["jobSearch"];
    let results = search["results"]
        .as_array()
        .ok_or_else(|| IndeedError::BadResponse("no job results".into()))?;
    let jobs = results
        .iter()
        .filter_map(|result| parse_indeed_job(&result["job"], host))
        .collect();
    let next_cursor = search["pageInfo"]["nextCursor"]
        .as_str()
        .filter(|cursor| !cursor.is_empty())
        .map(str::to_string);
    Ok(IndeedPage { jobs, next_cursor })
}

fn parse_indeed_job(job: &serde_json::Value, host: &str) -> Option<IndeedJob> {
    let text = |value: &serde_json::Value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let key = text(&job["key"]).filter(|key| key.chars().all(|c| c.is_ascii_alphanumeric()))?;
    let title = text(&job["title"])?;
    let html = text(&job["description"]["html"])?;
    let description = block_text(Html::parse_fragment(&html).root_element());
    if description.is_empty() {
        return None;
    }

    let employer = &job["employer"];
    let company = text(&job["sourceEmployerName"])
        .or_else(|| text(&employer["name"]))
        .unwrap_or_default();
    let company_url = text(&employer["relativeCompanyPageUrl"])
        .filter(|path| path.starts_with('/'))
        .map(|path| format!("https://{host}{path}"));
    let company_website = text(&employer["dossier"]["links"]["corporateWebsite"])
        .and_then(|site| reqwest::Url::parse(&site).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.to_string());

    let place = text(&job["location"]["formatted"]["long"]).or_else(|| text(&job["location"]["city"]));
    let country = text(&job["location"]["countryName"]);
    let location = match (place, country) {
        (Some(place), Some(country)) if !place.contains(&country) => format!("{place}, {country}"),
        (Some(place), _) => place,
        (None, Some(country)) => country,
        (None, None) => String::new(),
    };
    let posted_on = job["dateOnIndeed"]
        .as_i64()
        .and_then(DateTime::from_timestamp_millis)
        .map(|time| time.format("%Y-%m-%d").to_string());

    let attributes: Vec<(&str, &str)> = job["attributes"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| Some((item["key"].as_str()?, item["label"].as_str()?)))
                .collect()
        })
        .unwrap_or_default();
    let has = |wanted: &str| attributes.iter().any(|(key, _)| *key == wanted);
    let work_mode = if has(INDEED_REMOTE_KEY) {
        Some(WorkMode::Remote)
    } else if has(INDEED_HYBRID_KEY) {
        Some(WorkMode::Hybrid)
    } else {
        None
    };
    let job_types: Vec<&str> = attributes
        .iter()
        .filter(|(key, _)| INDEED_JOB_TYPE_KEYS.contains(key))
        .map(|(_, label)| *label)
        .collect();

    Some(IndeedJob {
        card: JobCard {
            source: Site::Indeed,
            url: format!("https://{host}/viewjob?jk={key}"),
            source_id: key,
            title,
            company,
            company_url,
            location,
            posted_on,
        },
        detail: JobDetail {
            description,
            seniority: None,
            employment_type: (!job_types.is_empty()).then(|| job_types.join(", ")),
            industries: text(&employer["dossier"]["employerDetails"]["industry"]),
        },
        work_mode,
        company_website,
    })
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

    pub async fn job_detail(&self, source_id: &str) -> Result<Option<JobDetail>, LinkedInError> {
        match self
            .get("detail", &format!("{DETAIL_URL}/{source_id}"), &[], None)
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
            if !cards.iter().any(|c| c.source_id == parsed.source_id) {
                cards.push(parsed);
            }
        }
    }
    cards
}

fn parse_card(card: ElementRef) -> Option<JobCard> {
    let urn = card.value().attr("data-entity-urn")?;
    let source_id = urn.rsplit(':').next()?.to_string();
    if source_id.is_empty() || !source_id.chars().all(|c| c.is_ascii_digit()) {
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
        .unwrap_or_else(|| format!("https://www.linkedin.com/jobs/view/{source_id}"));

    Some(JobCard {
        source: Site::LinkedIn,
        source_id,
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
            source: Site::LinkedIn,
            source_id: id.into(),
            title: title.into(),
            company: "Acme".into(),
            company_url: None,
            location: "Dubai".into(),
            posted_on: None,
            url: format!("https://www.linkedin.com/jobs/view/{id}"),
        }
    }

    fn indeed_body(cursor: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "data": { "jobSearch": {
                "pageInfo": { "nextCursor": cursor },
                "results": [
                    { "job": {
                        "key": "1b7c87ebd61932f5",
                        "title": "Senior Backend Engineer",
                        "sourceEmployerName": "Acme",
                        "dateOnIndeed": 1793076404324_i64,
                        "description": { "html": "<p>We use <b>Rust</b> and Kafka.</p><ul><li>Remote friendly</li></ul>" },
                        "location": { "city": "Dubai", "countryName": "UAE", "formatted": { "long": "Dubai" } },
                        "attributes": [
                            { "key": "CF3CP", "label": "Full-time" },
                            { "key": "DSQF7", "label": "Remote" }
                        ],
                        "employer": {
                            "name": "Acme LLC",
                            "relativeCompanyPageUrl": "/cmp/Acme",
                            "dossier": { "employerDetails": { "industry": "Software" }, "links": { "corporateWebsite": "https://acme.example/" } }
                        }
                    } },
                    { "job": {
                        "key": "abc123",
                        "title": "Painter Helper",
                        "description": { "html": "<p>Rust removal and painting.</p>" },
                        "location": { "city": "Dubai" },
                        "attributes": [],
                        "employer": null
                    } },
                    { "job": { "key": "nodesc", "title": "Backend Engineer", "description": { "html": "" } } }
                ]
            } }
        })
    }

    #[test]
    fn indeed_jobs_are_parsed() {
        let page = parse_indeed_page(&indeed_body(Some("next")), "ae.indeed.com").unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("next"));
        assert_eq!(page.jobs.len(), 2);
        let job = &page.jobs[0];
        assert_eq!(job.card.source, Site::Indeed);
        assert_eq!(job.card.source_id, "1b7c87ebd61932f5");
        assert_eq!(job.card.company, "Acme");
        assert_eq!(job.card.company_url.as_deref(), Some("https://ae.indeed.com/cmp/Acme"));
        assert_eq!(job.card.location, "Dubai, UAE");
        assert_eq!(job.card.posted_on.as_deref(), Some("2026-10-27"));
        assert_eq!(job.card.url, "https://ae.indeed.com/viewjob?jk=1b7c87ebd61932f5");
        assert!(job.detail.description.contains("Rust"));
        assert!(!job.detail.description.contains('<'));
        assert_eq!(job.detail.employment_type.as_deref(), Some("Full-time"));
        assert_eq!(job.detail.industries.as_deref(), Some("Software"));
        assert_eq!(job.work_mode, Some(WorkMode::Remote));
        assert_eq!(job.company_website.as_deref(), Some("https://acme.example/"));
        assert_eq!(page.jobs[1].company_website, None);
    }

    #[test]
    fn indeed_errors_are_reported() {
        let body = serde_json::json!({ "errors": [{ "message": "bad query" }] });
        assert!(matches!(
            parse_indeed_page(&body, "ae.indeed.com"),
            Err(IndeedError::BadResponse(message)) if message == "bad query"
        ));
    }

    #[test]
    fn indeed_country_comes_from_location() {
        let code = |place: &str| indeed_country(Some(place)).map(|country| country.code);
        assert_eq!(code("Dubai"), Some("AE"));
        assert_eq!(code("Dubai, United Arab Emirates"), Some("AE"));
        assert_eq!(code("London, UK"), Some("GB"));
        assert_eq!(code("Austin, Texas, United States"), Some("US"));
        assert_eq!(code("Atlantis"), None);
        assert_eq!(indeed_country(None), None);
        assert_eq!(indeed_country(Some("Berlin")).unwrap().host, "de.indeed.com");
    }

    struct FakeIndeed {
        calls: Mutex<Vec<(String, Option<String>, &'static str, bool)>>,
    }

    impl IndeedSource for FakeIndeed {
        fn search(
            &self,
            query: &IndeedQuery<'_>,
        ) -> impl Future<Output = Result<IndeedPage, IndeedError>> + Send {
            self.calls.lock().unwrap().push((
                query.keywords.to_string(),
                query.cursor.map(str::to_string),
                query.country.code,
                query.remote_only,
            ));
            let result = parse_indeed_page(&indeed_body(Some("more")), query.country.host);
            async move { result }
        }
    }

    #[tokio::test]
    async fn indeed_jobs_are_matched_and_stored() {
        let (store, dir) = setup(vec![WorkMode::Remote]);
        let indeed = FakeIndeed { calls: Mutex::new(Vec::new()) };
        let mut new_matches = 0;
        let report = scrape_indeed(&store, &indeed, &fast(), |search, jobs| {
            new_matches += crate::match_stage::apply_batch(&store, search, jobs).unwrap();
        })
        .await;

        assert_eq!(report.error, None);
        assert_eq!(report.requests, MAX_INDEED_PAGES_PER_QUERY);
        let calls = indeed.calls.lock().unwrap().clone();
        assert_eq!(calls[0], ("Backend Engineer".into(), None, "AE", true));
        assert_eq!(calls[1].1.as_deref(), Some("more"));
        assert_eq!(new_matches, 1);
        let jobs = store.list_jobs(None).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].source, Site::Indeed);
        assert_eq!(jobs[0].work_mode, Some(WorkMode::Remote));
        assert_eq!(jobs[0].company_website.as_deref(), Some("https://acme.example/"));
        assert!(store.companies_missing_site(8).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn indeed_skips_unknown_countries() {
        let dir = crate::db::test_dir("tailor-scan-test");
        let store = Db::open(&dir).unwrap();
        let criteria = SearchInput {
            name: None,
            titles: vec!["Backend Engineer".into()],
            keywords: vec![],
            location: Some("Atlantis".into()),
            work_modes: vec![WorkMode::Onsite],
            posted_within: PostedWithin::Week,
        }
        .validate()
        .unwrap();
        store.create_search(&criteria).unwrap();
        let indeed = FakeIndeed { calls: Mutex::new(Vec::new()) };
        let report = scrape_indeed(&store, &indeed, &fast(), |_, _| {}).await;
        assert_eq!(report.unknown_locations, vec!["Atlantis".to_string()]);
        assert!(indeed.calls.lock().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
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
                .map(|j| j.source_id.as_str())
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
            .map(|j| j.source_id)
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
                source: Site::LinkedIn,
                source_id: "4362132219".into(),
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
