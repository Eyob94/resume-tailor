CREATE TABLE users (
  id BIGSERIAL PRIMARY KEY,
  username TEXT UNIQUE,
  password_hash TEXT,
  created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX users_username_case_insensitive ON users (LOWER(username)) WHERE username IS NOT NULL;
CREATE TABLE sessions (
  token_hash TEXT PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at BIGINT NOT NULL
);
CREATE TABLE master_resume (
  user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  file_name TEXT NOT NULL,
  size_bytes BIGINT NOT NULL,
  page_count BIGINT NOT NULL,
  extracted_text TEXT NOT NULL,
  pdf BYTEA NOT NULL,
  uploaded_at TEXT NOT NULL
);
CREATE TABLE job_searches (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  titles TEXT NOT NULL,
  keywords TEXT NOT NULL,
  location TEXT,
  work_modes TEXT NOT NULL,
  posted_within TEXT NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE jobs (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  source TEXT NOT NULL DEFAULT 'linkedin',
  source_id TEXT NOT NULL,
  title TEXT NOT NULL,
  company TEXT NOT NULL,
  company_url TEXT,
  location TEXT NOT NULL,
  posted_on TEXT,
  url TEXT NOT NULL,
  description TEXT,
  seniority TEXT,
  employment_type TEXT,
  industries TEXT,
  work_mode TEXT,
  status TEXT NOT NULL DEFAULT 'new',
  tailored_at TEXT,
  first_seen_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (user_id, source_id)
);
CREATE TABLE search_matches (
  search_id BIGINT NOT NULL REFERENCES job_searches(id) ON DELETE CASCADE,
  job_id BIGINT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
  matched_keywords TEXT NOT NULL,
  matched_at TEXT NOT NULL,
  PRIMARY KEY (search_id, job_id)
);
CREATE TABLE settings (
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (user_id, key)
);
CREATE TABLE scan_runs (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  site TEXT NOT NULL DEFAULT 'linkedin',
  started_at TEXT NOT NULL,
  finished_at TEXT,
  status TEXT NOT NULL,
  new_matches BIGINT NOT NULL DEFAULT 0,
  message TEXT
);
CREATE TABLE resume_profiles (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  resume_uploaded_at TEXT NOT NULL,
  status TEXT NOT NULL,
  model TEXT NOT NULL,
  data TEXT,
  warnings TEXT NOT NULL DEFAULT '[]',
  error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  confirmed_at TEXT
);
CREATE TABLE job_scores (
  job_id BIGINT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
  profile_id BIGINT NOT NULL,
  status TEXT NOT NULL,
  score BIGINT,
  summary TEXT,
  requirements TEXT NOT NULL DEFAULT '[]',
  model TEXT NOT NULL,
  error TEXT,
  updated_at TEXT NOT NULL
);
CREATE TABLE companies (
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  url TEXT NOT NULL,
  website TEXT,
  fetched_at TEXT NOT NULL,
  PRIMARY KEY (user_id, url)
);
CREATE INDEX jobs_user_status ON jobs (user_id, status);
CREATE INDEX scan_runs_user_site ON scan_runs (user_id, site, id DESC);
CREATE INDEX resume_profiles_user ON resume_profiles (user_id, id DESC);
