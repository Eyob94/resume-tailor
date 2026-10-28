use std::{
    sync::Arc,
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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool, Postgres};

use crate::{errors::AppError, AppState, UserServices};

const SESSION_COOKIE: &str = "resume_tailor_session";
const SESSION_SECONDS: u64 = 60 * 60 * 24 * 30;

#[derive(Clone)]
pub struct AuthStore(PgPool);

#[derive(Debug)]
pub enum AuthError {
    InvalidCredentials,
    UserExists,
    Database(sqlx::Error),
    PasswordHash,
}

impl AuthStore {
    pub fn new(pool: PgPool) -> Self {
        Self(pool)
    }

    pub async fn register(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(i64, String), AuthError> {
        let salt = SaltString::generate(&mut OsRng);
        let password_hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| AuthError::PasswordHash)?
            .to_string();
        let mut tx = self.0.begin().await.map_err(AuthError::Database)?;
        let user_id = sqlx::query_scalar::<_, i64>("INSERT INTO users (username, password_hash, created_at) VALUES ($1, $2, $3) RETURNING id")
            .bind(username).bind(password_hash).bind(chrono::Utc::now().to_rfc3339()).fetch_one(&mut *tx).await.map_err(|error| {
                if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23505") { AuthError::UserExists } else { AuthError::Database(error) }
            })?;
        let token = create_session(&mut *tx, user_id).await?;
        tx.commit().await.map_err(AuthError::Database)?;
        Ok((user_id, token))
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<(i64, String), AuthError> {
        let user = sqlx::query_as::<_, (i64, Option<String>)>(
            "SELECT id, password_hash FROM users WHERE LOWER(username) = LOWER($1)",
        )
        .bind(username)
        .fetch_optional(&self.0)
        .await
        .map_err(AuthError::Database)?
        .ok_or(AuthError::InvalidCredentials)?;
        let password_hash = user.1.ok_or(AuthError::InvalidCredentials)?;
        let parsed = PasswordHash::new(&password_hash).map_err(|_| AuthError::PasswordHash)?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| AuthError::InvalidCredentials)?;
        let token = create_session(&self.0, user.0).await?;
        Ok((user.0, token))
    }

    pub async fn username(&self, user_id: i64) -> Result<Option<String>, AuthError> {
        sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&self.0)
            .await
            .map_err(AuthError::Database)
    }

    pub async fn session_user(&self, token: &str) -> Result<Option<i64>, AuthError> {
        let token_hash = hash_token(token);
        sqlx::query_scalar("SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > $2")
            .bind(token_hash)
            .bind(unix_time())
            .fetch_optional(&self.0)
            .await
            .map_err(AuthError::Database)
    }

    pub async fn logout(&self, token: &str) -> Result<(), AuthError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(hash_token(token))
            .execute(&self.0)
            .await
            .map(|_| ())
            .map_err(AuthError::Database)
    }
}

async fn create_session<'e>(
    connection: impl Executor<'e, Database = Postgres>,
    user_id: i64,
) -> Result<String, AuthError> {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(hash_token(&token))
        .bind(user_id)
        .bind(unix_time() + SESSION_SECONDS as i64)
        .execute(connection)
        .await
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
    username: Option<String>,
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
        .await
        .map_err(auth_error)?;
    let response = Json(UserResponse {
        username: Some(username.to_owned()),
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
        .await
        .map_err(auth_error)?;
    let username = state.auth.username(user_id).await.map_err(auth_error)?;
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
        state.auth.logout(&token).await.map_err(auth_error)?;
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
        .await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../tailor-core/migrations")]
    async fn password_authentication_uses_postgres_sessions(pool: PgPool) {
        let auth = AuthStore::new(pool);
        let (user_id, token) = auth
            .register("example-user", "correct-horse-battery")
            .await
            .unwrap();
        assert_eq!(
            auth.username(user_id).await.unwrap().as_deref(),
            Some("example-user")
        );
        assert_eq!(auth.session_user(&token).await.unwrap(), Some(user_id));
        let (login_id, login_token) = auth
            .login("EXAMPLE-USER", "correct-horse-battery")
            .await
            .unwrap();
        assert_eq!(login_id, user_id);
        assert_eq!(
            auth.session_user(&login_token).await.unwrap(),
            Some(user_id)
        );
        assert!(matches!(
            auth.login("example-user", "wrong-password").await,
            Err(AuthError::InvalidCredentials)
        ));
        auth.logout(&login_token).await.unwrap();
        assert_eq!(auth.session_user(&login_token).await.unwrap(), None);
    }
}
