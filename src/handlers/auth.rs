use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core::OsRng;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Redirect},
};
use axum_extra::extract::{
    cookie::{Cookie, SameSite},
    CookieJar,
};
use axum::Form;
use serde::Deserialize;
use std::sync::Arc;
use time::Duration;
use uuid::Uuid;

use crate::{
    db,
    models::{SessionData, User},
    AppState,
};

const SESSION_COOKIE_NAME: &str = "geo_session";

/// Retorna os dados da sessão a partir do cookie, se existir e for válida.
pub fn get_session(jar: &CookieJar, state: &AppState) -> Option<SessionData> {
    let session_id = jar.get(SESSION_COOKIE_NAME)?.value().to_string();
    state.sessions.lock().unwrap().get(&session_id).cloned()
}

fn build_session_cookie(session_id: String) -> Cookie<'static> {
    let secure = std::env::var("SESSION_SECURE")
        .map(|v| v == "true")
        .unwrap_or(false);
    let mut c = Cookie::new(SESSION_COOKIE_NAME, session_id);
    c.set_http_only(true);
    c.set_same_site(SameSite::Strict);
    c.set_path("/");
    c.set_max_age(Duration::days(7));
    c.set_secure(secure);
    c
}

// ─── GET /register ──────────────────────────────────────────────────────────

pub async fn register_page(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    if get_session(&jar, &state).is_some() {
        return Redirect::to("/").into_response();
    }
    let html = state
        .tmpl
        .get_template("register.html")
        .and_then(|t| t.render(minijinja::context! { error => "" }))
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── POST /register ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RegisterForm {
    pub email: String,
    pub password: String,
    pub password_confirm: String,
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Form(form): Form<RegisterForm>,
) -> impl IntoResponse {
    fn render_error(state: &AppState, msg: &str) -> axum::response::Response {
        let html = state
            .tmpl
            .get_template("register.html")
            .and_then(|t| t.render(minijinja::context! { error => msg }))
            .unwrap_or_else(|e| format!("Template error: {e}"));
        Html(html).into_response()
    }

    let email = form.email.trim().to_lowercase();

    // Validações básicas
    if email.is_empty() || !email.contains('@') || !email.contains('.') {
        return render_error(&state, "E-mail inválido.");
    }
    if form.password.len() < 8 {
        return render_error(&state, "A senha deve ter pelo menos 8 caracteres.");
    }
    if form.password != form.password_confirm {
        return render_error(&state, "As senhas não coincidem.");
    }

    // Verificar unicidade do e-mail
    match db::get_user_by_email(&state.pool, &email).await {
        Ok(Some(_)) => return render_error(&state, "Este e-mail já está cadastrado."),
        Err(e) => {
            tracing::error!("register: DB error: {e}");
            return render_error(&state, "Erro interno. Tente novamente.");
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
            return render_error(&state, "Erro ao processar senha. Tente novamente.");
        }
        Err(e) => {
            tracing::error!("register: spawn_blocking error: {e}");
            return render_error(&state, "Erro interno. Tente novamente.");
        }
    };

    // Criar usuário no banco
    let user = User::new(email.clone(), password_hash);
    if let Err(e) = db::insert_user(&state.pool, &user).await {
        tracing::error!("register: insert_user error: {e}");
        return render_error(&state, "Erro ao criar conta. Tente novamente.");
    }

    // Criar sessão e redirecionar
    let session_id = Uuid::new_v4().to_string();
    {
        let mut sessions = state.sessions.lock().unwrap();
        sessions.insert(
            session_id.clone(),
            SessionData {
                user_id: user.id.clone(),
                email: user.email.clone(),
            },
        );
    }

    tracing::info!("register: novo usuário criado: {}", email);
    (jar.add(build_session_cookie(session_id)), Redirect::to("/")).into_response()
}

// ─── GET /login ─────────────────────────────────────────────────────────────

pub async fn login_page(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    if get_session(&jar, &state).is_some() {
        return Redirect::to("/").into_response();
    }
    let html = state
        .tmpl
        .get_template("login.html")
        .and_then(|t| t.render(minijinja::context! { error => "", next => "" }))
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
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    let email = form.email.trim().to_lowercase();
    let next = form
        .next
        .as_deref()
        .filter(|s| !s.is_empty() && s.starts_with('/'))
        .unwrap_or("/")
        .to_string();

    fn render_error(state: &AppState, email: &str, next: &str, msg: &str) -> axum::response::Response {
        let html = state
            .tmpl
            .get_template("login.html")
            .and_then(|t| {
                t.render(minijinja::context! {
                    error => msg,
                    email => email,
                    next  => next,
                })
            })
            .unwrap_or_else(|e| format!("Template error: {e}"));
        Html(html).into_response()
    }

    // Buscar usuário pelo e-mail
    let user = match db::get_user_by_email(&state.pool, &email).await {
        Ok(Some(u)) => u,
        Ok(None) => return render_error(&state, &email, &next, "E-mail ou senha incorretos."),
        Err(e) => {
            tracing::error!("login: DB error: {e}");
            return render_error(&state, &email, &next, "Erro interno. Tente novamente.");
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
        return render_error(&state, &email, &next, "E-mail ou senha incorretos.");
    }

    // Criar sessão e redirecionar
    let session_id = Uuid::new_v4().to_string();
    {
        let mut sessions = state.sessions.lock().unwrap();
        sessions.insert(
            session_id.clone(),
            SessionData {
                user_id: user.id.clone(),
                email: user.email.clone(),
            },
        );
    }

    tracing::info!("login: usuário autenticado: {}", email);
    (jar.add(build_session_cookie(session_id)), Redirect::to(&next)).into_response()
}

// ─── POST /logout ────────────────────────────────────────────────────────────

pub async fn logout(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    // Remover sessão do store em memória
    if let Some(cookie) = jar.get(SESSION_COOKIE_NAME) {
        let session_id = cookie.value().to_string();
        state.sessions.lock().unwrap().remove(&session_id);
        tracing::info!("logout: sessão {} encerrada", session_id);
    }

    // Expirar cookie no browser (max-age=0, path=/)
    let mut removal = Cookie::new(SESSION_COOKIE_NAME, "");
    removal.set_path("/");
    removal.set_max_age(Duration::ZERO);
    removal.set_http_only(true);
    removal.set_same_site(SameSite::Strict);

    (jar.add(removal), Redirect::to("/login")).into_response()
}
