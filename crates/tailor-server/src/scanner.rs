//! Background job scanner: runs every saved search on a configurable interval.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Serialize;
use tailor_core::{
    linkedin::LinkedInClient,
    scan::{run_scan, ScanLimits},
    store::{ScanRun, Store, StoreError},
};
use tokio::sync::Notify;

pub const MIN_INTERVAL_MINUTES: u32 = 10;
pub const MAX_INTERVAL_MINUTES: u32 = 60;
pub const DEFAULT_INTERVAL_MINUTES: u32 = 60;
const INTERVAL_KEY: &str = "scan_interval_minutes";
/// Minimum gap between a scan and a manual "Run now". Override with
/// `SCAN_COOLDOWN_SECONDS` for local testing only.
const DEFAULT_COOLDOWN_SECONDS: i64 = MIN_INTERVAL_MINUTES as i64 * 60;

#[derive(Debug, Clone, Serialize)]
pub struct ScanStatus {
    pub running: bool,
    pub interval_minutes: u32,
    pub next_run_at: Option<String>,
    pub last_run: Option<ScanRun>,
}

#[derive(Default)]
struct State {
    running: bool,
    run_requested: bool,
    next_run_at: Option<DateTime<Utc>>,
}

pub struct Scanner {
    store: Arc<Store>,
    client: LinkedInClient,
    limits: ScanLimits,
    cooldown: ChronoDuration,
    state: Mutex<State>,
    wake: Notify,
}

impl Scanner {
    pub fn new(store: Arc<Store>, client: LinkedInClient) -> Self {
        let cooldown_seconds = std::env::var("SCAN_COOLDOWN_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_COOLDOWN_SECONDS);
        if cooldown_seconds != DEFAULT_COOLDOWN_SECONDS {
            tracing::warn!(cooldown_seconds, "manual scan cooldown overridden");
        }
        Self {
            store,
            client,
            limits: ScanLimits::default(),
            cooldown: ChronoDuration::seconds(cooldown_seconds),
            state: Mutex::default(),
            wake: Notify::new(),
        }
    }

    pub fn interval_minutes(&self) -> u32 {
        self.store
            .get_setting(INTERVAL_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .map(|m: u32| m.clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES))
            .unwrap_or(DEFAULT_INTERVAL_MINUTES)
    }

    /// Caller validates the range.
    pub fn set_interval_minutes(&self, minutes: u32) -> Result<(), StoreError> {
        self.store.set_setting(INTERVAL_KEY, &minutes.to_string())?;
        self.reschedule();
        Ok(())
    }

    pub fn status(&self) -> Result<ScanStatus, StoreError> {
        let state = self.state.lock().unwrap();
        Ok(ScanStatus {
            running: state.running,
            interval_minutes: self.interval_minutes(),
            next_run_at: state.next_run_at.map(|t| t.to_rfc3339()),
            last_run: self.store.last_scan_run()?,
        })
    }

    /// Searches or settings changed; recompute the schedule.
    pub fn reschedule(&self) {
        self.wake.notify_one();
    }

    /// Requests an immediate scan. Refused while one is running or within
    /// the cooldown of the last one (10 minutes unless overridden).
    pub fn request_run(&self) -> Result<(), String> {
        let last_start = self.last_started_at();
        let mut state = self.state.lock().unwrap();
        if state.running {
            return Err("A scan is already running.".into());
        }
        if let Some(last) = last_start {
            let ready_at = last + self.cooldown;
            if Utc::now() < ready_at {
                let wait = (ready_at - Utc::now()).num_seconds() + 1;
                let wait = if wait > 60 { format!("{} min", (wait + 59) / 60) } else { format!("{wait} s") };
                return Err(format!("A scan ran recently. Try again in {wait}."));
            }
        }
        state.run_requested = true;
        drop(state);
        self.wake.notify_one();
        Ok(())
    }

    fn last_started_at(&self) -> Option<DateTime<Utc>> {
        let run = self.store.last_scan_run().ok().flatten()?;
        DateTime::parse_from_rfc3339(&run.started_at).ok().map(|t| t.with_timezone(&Utc))
    }

    fn has_searches(&self) -> bool {
        self.store.list_searches().map(|s| !s.is_empty()).unwrap_or(false)
    }

    pub async fn run_forever(self: Arc<Self>) {
        if let Err(e) = self.store.mark_interrupted_scan_runs() {
            tracing::error!("couldn't tidy previous scan runs: {e}");
        }

        loop {
            let requested = std::mem::take(&mut self.state.lock().unwrap().run_requested);
            if !requested {
                if !self.has_searches() {
                    self.state.lock().unwrap().next_run_at = None;
                    self.wake.notified().await;
                    continue;
                }

                let interval = ChronoDuration::minutes(self.interval_minutes().into());
                let next = self.last_started_at().map(|t| t + interval).unwrap_or_else(Utc::now);
                self.state.lock().unwrap().next_run_at = Some(next);
                let wait = (next - Utc::now()).to_std().unwrap_or_default();
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = self.wake.notified() => continue,
                }
            }

            if self.has_searches() {
                self.scan_once().await;
            }
        }
    }

    async fn scan_once(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.running = true;
            state.next_run_at = None;
        }
        let run_id = match self.store.start_scan_run() {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("couldn't record scan start: {e}");
                self.state.lock().unwrap().running = false;
                // Avoid a hot loop if the database is broken.
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                return;
            }
        };

        tracing::info!("scan started");
        let report = run_scan(&self.store, &self.client, &self.limits).await;
        let status = match (&report.error, report.requests) {
            (None, _) => "ok",
            (Some(_), n) if n > 1 => "partial",
            (Some(_), _) => "failed",
        };
        let mut notes: Vec<String> = Vec::new();
        if let Some(error) = &report.error {
            notes.push(if error.ends_with('.') { error.clone() } else { format!("{error}.") });
        }
        if report.details_paused {
            notes.push(format!(
                "LinkedIn paused job page downloads; {} jobs will be completed next scan.",
                report.pending_details
            ));
        } else if report.pending_details > 0 {
            notes.push(format!("{} jobs still need details; continuing next scan.", report.pending_details));
        }
        let message = (!notes.is_empty()).then(|| notes.join(" "));
        tracing::info!(status, new_matches = report.new_matches, requests = report.requests, "scan finished");

        if let Err(e) = self.store.finish_scan_run(run_id, status, report.new_matches, message.as_deref()) {
            tracing::error!("couldn't record scan result: {e}");
        }
        self.state.lock().unwrap().running = false;
    }
}
