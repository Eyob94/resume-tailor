use chrono::Utc;
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

use super::Store;
use crate::errors::StoreError;
use crate::profile::{ResumeProfile, Warning};

fn now() -> String {
    Utc::now().to_rfc3339()
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

const COLUMNS: &str =
    "id, resume_uploaded_at, status, model, data, warnings, error, created_at, updated_at, confirmed_at";

fn from_row(row: &Row) -> rusqlite::Result<(ProfileRecord, Option<String>, String)> {
    Ok((
        ProfileRecord {
            id: row.get(0)?,
            resume_uploaded_at: row.get(1)?,
            status: row.get(2)?,
            model: row.get(3)?,
            profile: None,
            warnings: Vec::new(),
            error: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            confirmed_at: row.get(9)?,
        },
        row.get(4)?,
        row.get(5)?,
    ))
}

fn finish(parts: (ProfileRecord, Option<String>, String)) -> Result<ProfileRecord, StoreError> {
    let (mut record, data, warnings) = parts;
    record.profile = data.map(|d| serde_json::from_str(&d)).transpose()?;
    record.warnings = serde_json::from_str(&warnings)?;
    Ok(record)
}

impl Store {
    pub fn start_profile(&self, resume_uploaded_at: &str, model: &str) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let ts = now();
        conn.execute(
            "INSERT INTO resume_profiles (resume_uploaded_at, status, model, created_at, updated_at)
             VALUES (?1, 'reading', ?2, ?3, ?3)",
            params![resume_uploaded_at, model, ts],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finish_profile(&self, id: i64, profile: &ResumeProfile, warnings: &[Warning]) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE resume_profiles SET status = 'ready', data = ?2, warnings = ?3, error = NULL, updated_at = ?4
             WHERE id = ?1",
            params![id, serde_json::to_string(profile)?, serde_json::to_string(warnings)?, now()],
        )?;
        Ok(())
    }

    pub fn fail_profile(&self, id: i64, error: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE resume_profiles SET status = 'failed', error = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, error, now()],
        )?;
        Ok(())
    }

    pub fn latest_profile(&self, resume_uploaded_at: &str) -> Result<Option<ProfileRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let parts = conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM resume_profiles WHERE resume_uploaded_at = ?1 ORDER BY id DESC LIMIT 1"
                ),
                params![resume_uploaded_at],
                from_row,
            )
            .optional()?;
        parts.map(finish).transpose()
    }

    pub fn get_profile(&self, id: i64) -> Result<Option<ProfileRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let parts = conn
            .query_row(&format!("SELECT {COLUMNS} FROM resume_profiles WHERE id = ?1"), params![id], from_row)
            .optional()?;
        parts.map(finish).transpose()
    }

    pub fn save_profile_edits(
        &self,
        id: i64,
        profile: &ResumeProfile,
        warnings: &[Warning],
    ) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE resume_profiles SET status = 'ready', data = ?2, warnings = ?3, confirmed_at = NULL, updated_at = ?4
             WHERE id = ?1 AND status IN ('ready', 'confirmed')",
            params![id, serde_json::to_string(profile)?, serde_json::to_string(warnings)?, now()],
        )?;
        Ok(changed > 0)
    }

    pub fn confirm_profile(&self, id: i64) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let ts = now();
        let changed = conn.execute(
            "UPDATE resume_profiles SET status = 'confirmed', confirmed_at = ?2, updated_at = ?2
             WHERE id = ?1 AND status = 'ready'",
            params![id, ts],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_interrupted_profiles(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE resume_profiles SET status = 'failed', error = 'Interrupted by a restart. Try again.', updated_at = ?1
             WHERE status = 'reading'",
            params![now()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Contact;

    #[test]
    fn reading_lifecycle() {
        let dir = crate::store::test_dir("tailor-profile-test");
        let store = Store::open(&dir).unwrap();

        let id = store.start_profile("t1", "fake").unwrap();
        assert_eq!(store.latest_profile("t1").unwrap().unwrap().status, "reading");
        assert!(store.latest_profile("t2").unwrap().is_none());
        assert!(!store.confirm_profile(id).unwrap());

        let profile = ResumeProfile { contact: Contact { name: "Jordan".into(), ..Default::default() }, ..Default::default() };
        let warnings = vec![Warning { item_id: "e1.b1".into(), message: "x".into() }];
        store.finish_profile(id, &profile, &warnings).unwrap();
        let record = store.latest_profile("t1").unwrap().unwrap();
        assert_eq!((record.status.as_str(), record.warnings.len()), ("ready", 1));
        assert_eq!(record.profile.unwrap().contact.name, "Jordan");

        assert!(store.confirm_profile(id).unwrap());
        assert!(store.get_profile(id).unwrap().unwrap().confirmed_at.is_some());

        assert!(store.save_profile_edits(id, &profile, &[]).unwrap());
        let record = store.get_profile(id).unwrap().unwrap();
        assert_eq!((record.status.as_str(), record.confirmed_at), ("ready", None));

        let other = store.start_profile("t1", "fake").unwrap();
        store.mark_interrupted_profiles().unwrap();
        let record = store.latest_profile("t1").unwrap().unwrap();
        assert_eq!((record.id, record.status.as_str()), (other, "failed"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
