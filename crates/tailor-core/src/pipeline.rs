use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::{
    db::Db,
    match_stage::{MatchMessage, MatchStage},
    parse_stage::{ParseMessage, ParseStage},
    resume_stage::{ResumeMessage, ResumeStage},
    score_stage::{ScoreMessage, ScoreStage},
    scrape_stage::{LinkedInClient, ScrapeMessage, ScrapeStage},
};

pub struct PipelineConfig {
    pub ollama_url: String,
    pub scan_cooldown_seconds: i64,
}

#[derive(Clone)]
pub struct PipelineSenders {
    pub resume: UnboundedSender<ResumeMessage>,
    pub parse: UnboundedSender<ParseMessage>,
    pub scrape: UnboundedSender<ScrapeMessage>,
    pub score: UnboundedSender<ScoreMessage>,
}

pub struct Pipeline {
    resume_stage: ResumeStage,
    parse_stage: ParseStage,
    scrape_stage: ScrapeStage,
    match_stage: MatchStage,
    score_stage: ScoreStage,
}

impl Pipeline {
    pub fn new(
        db: Arc<Db>,
        config: PipelineConfig,
    ) -> Result<(Self, PipelineSenders), reqwest::Error> {
        let client = LinkedInClient::new()?;
        let (match_tx, match_rx) = tokio::sync::mpsc::unbounded_channel::<MatchMessage>();
        let (scrape_tx, scrape_rx) = tokio::sync::mpsc::unbounded_channel::<ScrapeMessage>();
        let (parse_tx, parse_rx) = tokio::sync::mpsc::unbounded_channel::<ParseMessage>();
        let (resume_tx, resume_rx) = tokio::sync::mpsc::unbounded_channel::<ResumeMessage>();
        let (score_tx, score_rx) = tokio::sync::mpsc::unbounded_channel::<ScoreMessage>();

        let score_stage = ScoreStage::new(db.clone(), config.ollama_url.clone(), score_rx);
        let match_stage = MatchStage::new(db.clone(), match_rx, score_tx.clone());
        let scrape_stage = ScrapeStage::new(
            db.clone(),
            client,
            config.scan_cooldown_seconds,
            scrape_rx,
            match_tx,
        );
        let parse_stage = ParseStage::new(db.clone(), config.ollama_url, parse_rx);
        let resume_stage = ResumeStage::new(db, resume_rx, parse_tx.clone());

        let senders = PipelineSenders {
            resume: resume_tx,
            parse: parse_tx,
            scrape: scrape_tx,
            score: score_tx,
        };
        let pipeline = Self {
            resume_stage,
            parse_stage,
            scrape_stage,
            match_stage,
            score_stage,
        };
        Ok((pipeline, senders))
    }

    pub async fn join(self) {
        self.resume_stage.join().await;
        self.parse_stage.join().await;
        self.scrape_stage.join().await;
        self.match_stage.join().await;
        self.score_stage.join().await;
    }
}
