use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core::OsRng;
use axum::{
    extract::{FromRequestParts, Query, State},
    http::request::Parts,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use axum::Form;
use serde::Deserialize;
use std::sync::Arc;
use tower_sessions::Session;

use crate::{
    db,
    models::{SessionData, User},
    AppState,
};

use super::csrf;

// ─── AuthUser extractor ──────────────────────────────────────────────────────

/// Extractor que valida a sessão autenticada.
/// Em rotas protegidas, se o usuário não estiver autenticado, redireciona para `/login?next=<path>`.
pub struct AuthUser(pub SessionData);

#[async_trait::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for AuthUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|e| e.into_response())?;

        match get_session_data(&session).await {
            Some(data) => Ok(AuthUser(data)),
            None => {
                let path = parts
                    .uri
                    .path_and_query()
                    .map(|pq| pq.as_str())
                    .unwrap_or("/");
                let next_param: String =
                    url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("next", path)
                        .finish();
                Err(Redirect::to(&format!("/login?{}", next_param)).into_response())
            }
        }
    }
}

const USER_ID_KEY: &str = "user_id";
const EMAIL_KEY: &str = "email";

/// Retorna os dados da sessão autenticada, se existir.
pub async fn get_session_data(session: &Session) -> Option<SessionData> {
    let user_id = session.get::<String>(USER_ID_KEY).await.ok()??;
    let email   = session.get::<String>(EMAIL_KEY).await.ok()??;
    Some(SessionData { user_id, email })
}

// ─── GET /register ──────────────────────────────────────────────────────────

pub async fn register_page(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> impl IntoResponse {
    if get_session_data(&session).await.is_some() {
        return Redirect::to("/").into_response();
    }
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;
    let html = state
        .tmpl
        .get_template("register.html")
        .and_then(|t| t.render(minijinja::context! { error => "", csrf_token => csrf_token }))
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── POST /register ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RegisterForm {
    pub email: String,
    pub password: String,
    pub password_confirm: String,
    pub csrf_token: Option<String>,
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    session: Session,
    Form(form): Form<RegisterForm>,
) -> impl IntoResponse {
    fn render_error(state: &AppState, msg: &str, csrf_token: &str, email: &str) -> axum::response::Response {
        let html = state
            .tmpl
            .get_template("register.html")
            .and_then(|t| t.render(minijinja::context! { error => msg, csrf_token => csrf_token, email => email }))
            .unwrap_or_else(|e| format!("Template error: {e}"));
        Html(html).into_response()
    }

    // Validação CSRF
    if !csrf::validate_csrf_token(&session, form.csrf_token.as_deref().unwrap_or("")).await {
        return (
            StatusCode::FORBIDDEN,
            Html("Token CSRF inválido. Recarregue a página e tente novamente.".to_string()),
        )
            .into_response();
    }

    // Gera (ou reutiliza) o token CSRF para re-renderização de erros
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;

    let email = form.email.trim().to_lowercase();

    // Validações básicas
    if email.is_empty() || !email.contains('@') || !email.contains('.') {
        return render_error(&state, "E-mail inválido.", &csrf_token, &email);
    }
    if form.password.len() < 8 {
        return render_error(&state, "A senha deve ter pelo menos 8 caracteres.", &csrf_token, &email);
    }
    if form.password != form.password_confirm {
        return render_error(&state, "As senhas não coincidem.", &csrf_token, &email);
    }

    // Verificar unicidade do e-mail
    match db::get_user_by_email(&state.pool, &email).await {
        Ok(Some(_)) => {
            // Exibe erro claro sem revelar se o e-mail pertence a outro usuário autenticado
            return render_error(&state, "Este e-mail já está cadastrado. Faça login ou use outro e-mail.", &csrf_token, &email);
        }
        Err(e) => {
            tracing::error!("register: DB error: {e}");
            return render_error(&state, "Erro interno. Tente novamente.", &csrf_token, &email);
        }
        Ok(None) => {}
    }

    // Hash da senha — operação CPU-bound, executa em thread separada
    let password = form.password.clone();
    let hash_result = tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| e.to_string())
    })
    .await;

    let password_hash = match hash_result {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::error!("register: argon2 hash error: {e}");
            return render_error(&state, "Erro ao processar senha. Tente novamente.", &csrf_token, &email);
        }
        Err(e) => {
            tracing::error!("register: spawn_blocking error: {e}");
            return render_error(&state, "Erro interno. Tente novamente.", &csrf_token, &email);
        }
    };

    // Criar usuário no banco
    let user = User::new(email.clone(), password_hash);
    if let Err(e) = db::insert_user(&state.pool, &user).await {
        tracing::error!("register: insert_user error: {e}");
        return render_error(&state, "Erro ao criar conta. Tente novamente.", &csrf_token, &email);
    }

    // Persistir sessão e redirecionar
    // Regenerar ID de sessão após autenticação para prevenir session fixation
    if let Err(e) = session.cycle_id().await {
        tracing::warn!("register: falha ao regenerar ID de sessão: {e}");
    }
    if let Err(e) = session.insert(USER_ID_KEY, &user.id).await {
        tracing::error!("register: session insert user_id error: {e}");
        return render_error(&state, "Erro interno. Tente novamente.", &csrf_token, &email);
    }
    if let Err(e) = session.insert(EMAIL_KEY, &user.email).await {
        tracing::error!("register: session insert email error: {e}");
        return render_error(&state, "Erro interno. Tente novamente.", &csrf_token, &email);
    }

    tracing::info!("register: novo usuário criado: {}", email);
    Redirect::to("/").into_response()
}

// ─── GET /login ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginQuery {
    pub next: Option<String>,
}

pub async fn login_page(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(q): Query<LoginQuery>,
) -> impl IntoResponse {
    if get_session_data(&session).await.is_some() {
        return Redirect::to("/").into_response();
    }
    let next = q
        .next
        .as_deref()
        .filter(|s| s.starts_with('/'))
        .unwrap_or("");
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;
    let html = state
        .tmpl
        .get_template("login.html")
        .and_then(|t| t.render(minijinja::context! { error => "", next => next, email => "", csrf_token => csrf_token }))
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── POST /login ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    /// URL para redirecionar após login bem-sucedido (?next=...)
    pub next: Option<String>,
    pub csrf_token: Option<String>,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    let email = form.email.trim().to_lowercase();
    let next = form
        .next
        .as_deref()
        .filter(|s| !s.is_empty() && s.starts_with('/'))
        .unwrap_or("/")
        .to_string();

    // Validação CSRF
    if !csrf::validate_csrf_token(&session, form.csrf_token.as_deref().unwrap_or("")).await {
        return (
            StatusCode::FORBIDDEN,
            Html("Token CSRF inválido. Recarregue a página e tente novamente.".to_string()),
        )
            .into_response();
    }

    // Gera (ou reutiliza) o token CSRF para re-renderização de erros
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;

    fn render_error(state: &AppState, email: &str, next: &str, msg: &str, csrf_token: &str) -> axum::response::Response {
        let html = state
            .tmpl
            .get_template("login.html")
            .and_then(|t| {
                t.render(minijinja::context! {
                    error => msg,
                    email => email,
                    next  => next,
                    csrf_token => csrf_token,
                })
            })
            .unwrap_or_else(|e| format!("Template error: {e}"));
        Html(html).into_response()
    }

    // Buscar usuário pelo e-mail
    let user = match db::get_user_by_email(&state.pool, &email).await {
        Ok(Some(u)) => u,
        Ok(None) => return render_error(&state, &email, &next, "E-mail ou senha incorretos.", &csrf_token),
        Err(e) => {
            tracing::error!("login: DB error: {e}");
            return render_error(&state, &email, &next, "Erro interno. Tente novamente.", &csrf_token);
        }
    };

    // Verificar senha — operação CPU-bound, executa em thread separada
    let stored_hash = user.password_hash.clone();
    let password = form.password.clone();
    let valid = tokio::task::spawn_blocking(move || {
        PasswordHash::new(&stored_hash)
            .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);

    if !valid {
        return render_error(&state, &email, &next, "E-mail ou senha incorretos.", &csrf_token);
    }

    // Persistir sessão
    // Regenerar ID de sessão após autenticação para prevenir session fixation
    if let Err(e) = session.cycle_id().await {
        tracing::warn!("login: falha ao regenerar ID de sessão: {e}");
    }
    if let Err(e) = session.insert(USER_ID_KEY, &user.id).await {
        tracing::error!("login: session insert user_id error: {e}");
        return render_error(&state, &email, &next, "Erro interno. Tente novamente.", &csrf_token);
    }
    if let Err(e) = session.insert(EMAIL_KEY, &user.email).await {
        tracing::error!("login: session insert email error: {e}");
        return render_error(&state, &email, &next, "Erro interno. Tente novamente.", &csrf_token);
    }

    tracing::info!("login: usuário autenticado: {}", email);
    Redirect::to(&next).into_response()
}

// ─── POST /logout ────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct LogoutForm {
    pub csrf_token: Option<String>,
}

pub async fn logout(session: Session, Form(form): Form<LogoutForm>) -> impl IntoResponse {
    if !csrf::validate_csrf_token(&session, form.csrf_token.as_deref().unwrap_or("")).await {
        return (
            StatusCode::FORBIDDEN,
            Html("Token CSRF inválido.".to_string()),
        )
            .into_response();
    }
    if let Err(e) = session.delete().await {
        tracing::warn!("logout: erro ao deletar sessão: {e}");
    }
    Redirect::to("/login").into_response()
}

