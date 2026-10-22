
use std::time::Duration;

use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use thiserror::Error;

use crate::search::{PostedWithin, WorkMode};

const SEARCH_PAGE_URL: &str = "https://www.linkedin.com/jobs/search";
const SEE_MORE_URL: &str = "https://www.linkedin.com/jobs-guest/jobs/api/seeMoreJobPostings/search";
const DETAIL_URL: &str = "https://www.linkedin.com/jobs/view";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

#[derive(Debug, Error)]
pub enum LinkedInError {
    #[error("LinkedIn is rate-limiting requests (HTTP {0}); will retry next scan")]
    Blocked(u16),
    #[error("LinkedIn returned HTTP {0}")]
    Status(u16),
    #[error("couldn't reach LinkedIn: {0}")]
    Network(#[from] reqwest::Error),
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
        headers.insert(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,*/*;q=0.8".parse().unwrap());
        headers.insert(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse().unwrap());
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
            self.get("search-more", SEE_MORE_URL, &params, referer.as_ref().map(reqwest::Url::as_str)).await?
        };
        Ok(parse_search_results(&html))
    }

    pub async fn job_detail(&self, linkedin_id: &str) -> Result<Option<JobDetail>, LinkedInError> {
        match self.get("detail", &format!("{DETAIL_URL}/{linkedin_id}"), &[], None).await {
            Ok(html) => Ok(parse_job_detail(&html)),
            Err(LinkedInError::Status(404 | 410)) => Ok(None),
            Err(e) => Err(e),
        }
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
        let start = params.iter().find(|(k, _)| *k == "start").map(|(_, v)| v.as_str()).unwrap_or("-");
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
    el.text().collect::<Vec<_>>().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
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
    let title = first(".base-search-card__title").map(clean_text).filter(|t| !t.is_empty())?;
    let company = first(".base-search-card__subtitle").map(clean_text).unwrap_or_default();
    let company_url = first(".base-search-card__subtitle a[href]").and_then(|a| a.value().attr("href")).map(without_query);
    let location = first(".job-search-card__location").map(clean_text).unwrap_or_default();
    let posted_on = first("time[datetime]").and_then(|t| t.value().attr("datetime")).map(str::to_string);
    let url = first("a.base-card__full-link[href]")
        .and_then(|a| a.value().attr("href"))
        .or_else(|| card.value().attr("href"))
        .map(without_query)
        .unwrap_or_else(|| format!("https://www.linkedin.com/jobs/view/{linkedin_id}"));

    Some(JobCard { linkedin_id, title, company, company_url, location, posted_on, url })
}

pub fn parse_job_detail(html: &str) -> Option<JobDetail> {
    let doc = Html::parse_document(html);
    let markup = doc.select(&selector(".show-more-less-html__markup")).next()?;
    let description = block_text(markup);
    if description.is_empty() {
        return None;
    }

    let mut detail = JobDetail { description, seniority: None, employment_type: None, industries: None };
    let header_sel = selector(".description__job-criteria-subheader");
    let value_sel = selector(".description__job-criteria-text");
    for item in doc.select(&selector(".description__job-criteria-item")) {
        let (Some(header), Some(value)) = (item.select(&header_sel).next(), item.select(&value_sel).next()) else {
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
                    "p" | "div" | "ul" | "ol" | "section" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
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
}
