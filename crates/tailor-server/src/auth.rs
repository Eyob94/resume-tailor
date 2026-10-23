use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{Extension, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{errors::AppError, AppState, UserServices};

const SESSION_COOKIE: &str = "resume_tailor_session";
const SESSION_SECONDS: u64 = 60 * 60 * 24 * 30;

#[derive(Clone)]
pub struct AuthStore(Arc<Mutex<Connection>>);

#[derive(Debug)]
pub enum AuthError {
    InvalidCredentials,
    UserExists,
    Database(rusqlite::Error),
    PasswordHash,
}

impl AuthStore {
    pub fn open(data_dir: &Path) -> rusqlite::Result<Self> {
        std::fs::create_dir_all(data_dir)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let connection = Connection::open(data_dir.join("auth.sqlite"))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS users (
                 id INTEGER PRIMARY KEY,
                 username TEXT NOT NULL UNIQUE COLLATE NOCASE,
                 password_hash TEXT NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 token_hash TEXT PRIMARY KEY,
                 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                 expires_at INTEGER NOT NULL
             );",
        )?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub fn register(&self, username: &str, password: &str) -> Result<(i64, String), AuthError> {
        let salt = SaltString::generate(&mut OsRng);
        let password_hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| AuthError::PasswordHash)?
            .to_string();
        let now = unix_time();
        let connection = self.0.lock().unwrap();
        connection
            .execute(
                "INSERT INTO users (username, password_hash, created_at) VALUES (?1, ?2, ?3)",
                params![username, password_hash, now],
            )
            .map_err(|error| {
                if matches!(error, rusqlite::Error::SqliteFailure(ref e, _)
                    if e.code == rusqlite::ErrorCode::ConstraintViolation)
                {
                    AuthError::UserExists
                } else {
                    AuthError::Database(error)
                }
            })?;
        let user_id = connection.last_insert_rowid();
        let token = create_session(&connection, user_id)?;
        Ok((user_id, token))
    }

    pub fn login(&self, username: &str, password: &str) -> Result<(i64, String), AuthError> {
        let connection = self.0.lock().unwrap();
        let user = connection
            .query_row(
                "SELECT id, password_hash FROM users WHERE username = ?1 COLLATE NOCASE",
                [username],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(AuthError::Database)?
            .ok_or(AuthError::InvalidCredentials)?;
        let parsed = PasswordHash::new(&user.1).map_err(|_| AuthError::PasswordHash)?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| AuthError::InvalidCredentials)?;
        let token = create_session(&connection, user.0)?;
        Ok((user.0, token))
    }

    pub fn username(&self, user_id: i64) -> Result<Option<String>, AuthError> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT username FROM users WHERE id = ?1",
                [user_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AuthError::Database)
    }

    pub fn session_user(&self, token: &str) -> Result<Option<i64>, AuthError> {
        let token_hash = hash_token(token);
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT user_id FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
                params![token_hash, unix_time()],
                |row| row.get(0),
            )
            .optional()
            .map_err(AuthError::Database)
    }

    pub fn logout(&self, token: &str) -> Result<(), AuthError> {
        self.0
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM sessions WHERE token_hash = ?1",
                [hash_token(token)],
            )
            .map(|_| ())
            .map_err(AuthError::Database)
    }
}

fn create_session(connection: &Connection, user_id: i64) -> Result<String, AuthError> {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    connection
        .execute(
            "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![
                hash_token(&token),
                user_id,
                unix_time() + SESSION_SECONDS as i64
            ],
        )
        .map_err(AuthError::Database)?;
    Ok(token)
}

fn hash_token(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct UserResponse {
    username: String,
}

pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
}

pub fn protected_routes() -> Router<AppState> {
    Router::new().route("/auth/me", get(me))
}

fn validate_credentials(credentials: &Credentials) -> Result<(), AppError> {
    let username = credentials.username.trim();
    if !(3..=32).contains(&username.len())
        || !username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(AppError::bad_request(
            "Username must be 3–32 characters using letters, numbers, dots, dashes, or underscores.",
        ));
    }
    if !(12..=1024).contains(&credentials.password.len()) {
        return Err(AppError::bad_request(
            "Password must be between 12 and 1024 characters.",
        ));
    }
    Ok(())
}

async fn register(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<impl IntoResponse, AppError> {
    validate_credentials(&credentials)?;
    let username = credentials.username.trim();
    let (_user_id, token) = state
        .auth
        .register(username, &credentials.password)
        .map_err(auth_error)?;
    let response = Json(UserResponse {
        username: username.to_owned(),
    });
    Ok((StatusCode::CREATED, with_session_cookie(response, &token)?))
}

async fn login(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<impl IntoResponse, AppError> {
    let (user_id, token) = state
        .auth
        .login(credentials.username.trim(), &credentials.password)
        .map_err(auth_error)?;
    let username = state
        .auth
        .username(user_id)
        .map_err(auth_error)?
        .ok_or_else(|| AppError::internal("user missing after login"))?;
    Ok(with_session_cookie(
        Json(UserResponse { username }),
        &token,
    )?)
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    if let Some(token) = cookie_token(&headers) {
        state.auth.logout(&token).map_err(auth_error)?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "resume_tailor_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        ),
    );
    Ok(response)
}

async fn me(Extension(user): Extension<Arc<UserServices>>) -> Json<UserResponse> {
    Json(UserResponse {
        username: user.username.clone(),
    })
}

fn with_session_cookie<T: IntoResponse>(response: T, token: &str) -> Result<Response, AppError> {
    let mut response = response.into_response();
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_SECONDS}"
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(AppError::internal)?,
    );
    Ok(response)
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == SESSION_COOKIE).then(|| value.to_owned())
        })
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut request: axum::extract::Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = cookie_token(request.headers()).ok_or_else(unauthorized)?;
    let user_id = state
        .auth
        .session_user(&token)
        .map_err(auth_error)?
        .ok_or_else(unauthorized)?;
    let user = state
        .user_services(user_id)
        .await
        .map_err(AppError::internal)?;
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

fn unauthorized() -> AppError {
    AppError {
        status: StatusCode::UNAUTHORIZED,
        message: "Please log in to continue.".into(),
    }
}

fn auth_error(error: AuthError) -> AppError {
    match error {
        AuthError::InvalidCredentials => AppError {
            status: StatusCode::UNAUTHORIZED,
            message: "Incorrect username or password.".into(),
        },
        AuthError::UserExists => AppError::bad_request("That username is already registered."),
        AuthError::Database(error) => AppError::internal(error),
        AuthError::PasswordHash => AppError::internal("password hashing failed"),
    }
}
