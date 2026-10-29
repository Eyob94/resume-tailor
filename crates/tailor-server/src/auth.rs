use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Extension, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rand::{rngs::OsRng, Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool, Postgres};

use crate::{errors::AppError, AppState, UserServices};

const SESSION_COOKIE: &str = "resume_tailor_session";
const SESSION_SECONDS: u64 = 60 * 60 * 24 * 30;
const KEY_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Clone)]
pub struct AuthStore(PgPool);

#[derive(Debug)]
pub enum AuthError {
    Database(sqlx::Error),
}

impl AuthStore {
    pub fn new(pool: PgPool) -> Self {
        Self(pool)
    }

    pub async fn create_key_account(&self) -> Result<(i64, String, String), AuthError> {
        let key = new_key();
        let mut tx = self.0.begin().await.map_err(AuthError::Database)?;
        let user_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO users (created_at, key_hash) VALUES ($1, $2) RETURNING id",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(hash_token(&key))
        .fetch_one(&mut *tx)
        .await
        .map_err(AuthError::Database)?;
        let token = create_session(&mut *tx, user_id).await?;
        tx.commit().await.map_err(AuthError::Database)?;
        Ok((user_id, key, token))
    }

    pub async fn key_login(&self, key: &str) -> Result<Option<(i64, String)>, AuthError> {
        let user_id = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE key_hash = $1")
            .bind(hash_token(&normalize_key(key)))
            .fetch_optional(&self.0)
            .await
            .map_err(AuthError::Database)?;
        match user_id {
            Some(user_id) => Ok(Some((user_id, create_session(&self.0, user_id).await?))),
            None => Ok(None),
        }
    }

    pub async fn rotate_key(&self, user_id: i64) -> Result<String, AuthError> {
        let key = new_key();
        sqlx::query("UPDATE users SET key_hash = $1 WHERE id = $2")
            .bind(hash_token(&key))
            .bind(user_id)
            .execute(&self.0)
            .await
            .map_err(AuthError::Database)?;
        Ok(key)
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
    OsRng.fill_bytes(&mut bytes);
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

fn new_key() -> String {
    let groups = (0..6)
        .map(|_| {
            (0..4)
                .map(|_| KEY_ALPHABET[OsRng.gen_range(0..KEY_ALPHABET.len())] as char)
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    format!("RT-{}", groups.join("-"))
}

fn normalize_key(key: &str) -> String {
    key.split_whitespace().collect::<String>().to_uppercase()
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
struct KeyLogin {
    key: String,
}

#[derive(Serialize)]
struct KeyResponse {
    key: String,
}

#[derive(Serialize)]
struct MeResponse {
    user_id: i64,
}

pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/key", post(create_key))
        .route("/auth/key/login", post(key_login))
        .route("/auth/logout", post(logout))
}

pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/me", get(me))
        .route("/auth/key/rotate", post(rotate_key))
}

async fn create_key(State(state): State<AppState>) -> Result<Response, AppError> {
    let (_user_id, key, token) = state
        .auth
        .create_key_account()
        .await
        .map_err(auth_error)?;
    with_session_cookie((StatusCode::CREATED, Json(KeyResponse { key })), &token)
}

async fn key_login(
    State(state): State<AppState>,
    Json(body): Json<KeyLogin>,
) -> Result<Response, AppError> {
    let (_user_id, token) = state
        .auth
        .key_login(&body.key)
        .await
        .map_err(auth_error)?
        .ok_or_else(|| AppError {
            status: StatusCode::UNAUTHORIZED,
            message: "That key doesn't match an account.".into(),
        })?;
    with_session_cookie(
        Json(KeyResponse {
            key: normalize_key(&body.key),
        }),
        &token,
    )
}

async fn rotate_key(
    State(state): State<AppState>,
    Extension(user): Extension<Arc<UserServices>>,
) -> Result<Json<KeyResponse>, AppError> {
    let key = state
        .auth
        .rotate_key(user.user_id)
        .await
        .map_err(auth_error)?;
    Ok(Json(KeyResponse { key }))
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

async fn me(Extension(user): Extension<Arc<UserServices>>) -> Json<MeResponse> {
    Json(MeResponse {
        user_id: user.user_id,
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
        AuthError::Database(error) => AppError::internal(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_key_format(key: &str) -> bool {
        let parts = key.split('-').collect::<Vec<_>>();
        parts.len() == 7
            && parts[0] == "RT"
            && parts[1..].iter().all(|part| {
                part.len() == 4 && part.bytes().all(|byte| KEY_ALPHABET.contains(&byte))
            })
    }

    #[sqlx::test(migrations = "../tailor-core/migrations")]
    async fn key_accounts_sign_back_in_with_their_key(pool: PgPool) {
        let auth = AuthStore::new(pool);
        let (user_id, key, token) = auth.create_key_account().await.unwrap();
        assert!(is_key_format(&key));
        assert_eq!(auth.session_user(&token).await.unwrap(), Some(user_id));
        let (login_id, login_token) = auth.key_login(&key).await.unwrap().unwrap();
        assert_eq!(login_id, user_id);
        assert_eq!(
            auth.session_user(&login_token).await.unwrap(),
            Some(user_id)
        );
        auth.logout(&login_token).await.unwrap();
        assert_eq!(auth.session_user(&login_token).await.unwrap(), None);
    }

    #[sqlx::test(migrations = "../tailor-core/migrations")]
    async fn keys_ignore_case_and_spaces(pool: PgPool) {
        let auth = AuthStore::new(pool);
        let (user_id, key, _) = auth.create_key_account().await.unwrap();
        let typed = format!("  {}  ", key.to_lowercase().replace('-', " - "));
        let (login_id, _) = auth.key_login(&typed).await.unwrap().unwrap();
        assert_eq!(login_id, user_id);
    }

    #[sqlx::test(migrations = "../tailor-core/migrations")]
    async fn wrong_key_is_rejected(pool: PgPool) {
        let auth = AuthStore::new(pool);
        auth.create_key_account().await.unwrap();
        assert!(auth
            .key_login("RT-0000-0000-0000-0000-0000-0000")
            .await
            .unwrap()
            .is_none());
    }

    #[sqlx::test(migrations = "../tailor-core/migrations")]
    async fn rotating_a_key_replaces_it(pool: PgPool) {
        let auth = AuthStore::new(pool);
        let (user_id, old_key, _) = auth.create_key_account().await.unwrap();
        let new_key = auth.rotate_key(user_id).await.unwrap();
        assert!(is_key_format(&new_key));
        assert_ne!(new_key, old_key);
        assert!(auth.key_login(&old_key).await.unwrap().is_none());
        let (login_id, _) = auth.key_login(&new_key).await.unwrap().unwrap();
        assert_eq!(login_id, user_id);
    }
}
