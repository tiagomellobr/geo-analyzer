use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Argon2,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    Form,
};
use chrono::{Duration, Utc};
use lettre::{
    message::header::ContentType, AsyncTransport, Message, AsyncSmtpTransport,
    Tokio1Executor,
};
use rand_core::OsRng;
use serde::Deserialize;
use std::sync::Arc;
use tower_sessions::Session;
use uuid::Uuid;

use crate::{db, AppState};
use super::csrf;

// ─── Configuração SMTP ────────────────────────────────────────────────────────

struct SmtpConfig {
    host: String,
    port: u16,
    user: String,
    pass: String,
    from: String,
    base_url: String,
}

impl SmtpConfig {
    fn from_env() -> Option<Self> {
        Some(Self {
            host: std::env::var("SMTP_HOST").ok()?,
            port: std::env::var("SMTP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(587),
            user: std::env::var("SMTP_USER").ok()?,
            pass: std::env::var("SMTP_PASS").ok()?,
            from: std::env::var("SMTP_FROM")
                .unwrap_or_else(|_| "noreply@geo-analyzer.app".to_string()),
            base_url: std::env::var("APP_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:3000".to_string()),
        })
    }
}

async fn send_reset_email(config: &SmtpConfig, to_email: &str, token: &str) -> anyhow::Result<()> {
    let reset_url = format!("{}/reset-password/{}", config.base_url, token);

    let body = format!(
        "Olá,\n\n\
        Recebemos uma solicitação para redefinir a senha da sua conta GEO Analyzer.\n\n\
        Clique no link abaixo (válido por 1 hora):\n\
        {reset_url}\n\n\
        Se você não solicitou isso, ignore este e-mail — sua senha permanece a mesma.\n\n\
        — GEO Analyzer"
    );

    let email = Message::builder()
        .from(config.from.parse()?)
        .to(to_email.parse()?)
        .subject("Redefinição de senha — GEO Analyzer")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;

    let creds = lettre::transport::smtp::authentication::Credentials::new(
        config.user.clone(),
        config.pass.clone(),
    );

    let mailer = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)?
        .port(config.port)
        .credentials(creds)
        .build();

    mailer.send(email).await?;
    Ok(())
}

// ─── GET /forgot-password ─────────────────────────────────────────────────────

pub async fn forgot_password_page(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> impl IntoResponse {
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;
    let html = state
        .tmpl
        .get_template("forgot_password.html")
        .and_then(|t| t.render(minijinja::context! { csrf_token => csrf_token, sent => false, error => "" }))
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html)
}

// ─── POST /forgot-password ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ForgotForm {
    pub email: String,
    pub csrf_token: Option<String>,
}

pub async fn forgot_password(
    State(state): State<Arc<AppState>>,
    session: Session,
    Form(form): Form<ForgotForm>,
) -> impl IntoResponse {
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;

    if !csrf::validate_csrf_token(&session, form.csrf_token.as_deref().unwrap_or("")).await {
        return (
            StatusCode::FORBIDDEN,
            Html("Token CSRF inválido. Recarregue a página e tente novamente.".to_string()),
        )
            .into_response();
    }

    let email = form.email.trim().to_lowercase();

    // Resposta sempre idêntica para não revelar se o e-mail existe (prevenção de enumeração)
    let sent_html = || {
        state
            .tmpl
            .get_template("forgot_password.html")
            .and_then(|t| t.render(minijinja::context! { csrf_token => &csrf_token, sent => true, error => "" }))
            .unwrap_or_else(|e| format!("Template error: {e}"))
    };

    let user = match db::get_user_by_email(&state.pool, &email).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            // Não revela que o e-mail não existe
            return Html(sent_html()).into_response();
        }
        Err(e) => {
            tracing::error!("forgot_password: DB error: {e}");
            return Html(sent_html()).into_response();
        }
    };

    let smtp = match SmtpConfig::from_env() {
        Some(c) => c,
        None => {
            tracing::warn!("forgot_password: variáveis SMTP não configuradas; e-mail não enviado");
            return Html(sent_html()).into_response();
        }
    };

    let token = Uuid::new_v4().to_string().replace('-', "");
    let expires_at = (Utc::now() + Duration::hours(1)).to_rfc3339();

    if let Err(e) = db::create_reset_token(&state.pool, &user.id, &token, &expires_at).await {
        tracing::error!("forgot_password: create_reset_token error: {e}");
        return Html(sent_html()).into_response();
    }

    if let Err(e) = send_reset_email(&smtp, &email, &token).await {
        tracing::error!("forgot_password: falha ao enviar e-mail para {}: {e}", email);
        // Mesmo com falha, mostra mensagem genérica (não expõe o erro ao usuário)
    } else {
        tracing::info!("forgot_password: e-mail de reset enviado para {}", email);
    }

    Html(sent_html()).into_response()
}

// ─── GET /reset-password/:token ───────────────────────────────────────────────

pub async fn reset_password_page(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(token): Path<String>,
) -> impl IntoResponse {
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;

    match validate_token(&state, &token).await {
        Ok(_) => {
            let html = state
                .tmpl
                .get_template("reset_password.html")
                .and_then(|t| t.render(minijinja::context! { token => &token, csrf_token => &csrf_token, error => "" }))
                .unwrap_or_else(|e| format!("Template error: {e}"));
            Html(html).into_response()
        }
        Err(msg) => {
            let html = state
                .tmpl
                .get_template("reset_password.html")
                .and_then(|t| t.render(minijinja::context! { token => "", csrf_token => &csrf_token, error => msg }))
                .unwrap_or_else(|e| format!("Template error: {e}"));
            Html(html).into_response()
        }
    }
}

// ─── POST /reset-password/:token ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ResetForm {
    pub password: String,
    pub password_confirm: String,
    pub csrf_token: Option<String>,
}

pub async fn reset_password(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(token): Path<String>,
    Form(form): Form<ResetForm>,
) -> impl IntoResponse {
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;

    if !csrf::validate_csrf_token(&session, form.csrf_token.as_deref().unwrap_or("")).await {
        return (
            StatusCode::FORBIDDEN,
            Html("Token CSRF inválido. Recarregue a página e tente novamente.".to_string()),
        )
            .into_response();
    }

    fn render_error(state: &AppState, token: &str, csrf_token: &str, msg: &str) -> axum::response::Response {
        let html = state
            .tmpl
            .get_template("reset_password.html")
            .and_then(|t| t.render(minijinja::context! { token => token, csrf_token => csrf_token, error => msg }))
            .unwrap_or_else(|e| format!("Template error: {e}"));
        Html(html).into_response()
    }

    // Validar token
    let user_id = match validate_token(&state, &token).await {
        Ok(uid) => uid,
        Err(msg) => return render_error(&state, "", &csrf_token, &msg),
    };

    // Validar senha
    if form.password.len() < 8 {
        return render_error(&state, &token, &csrf_token, "A senha deve ter pelo menos 8 caracteres.");
    }
    if form.password != form.password_confirm {
        return render_error(&state, &token, &csrf_token, "As senhas não coincidem.");
    }

    // Hash da senha — CPU-bound
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
        _ => return render_error(&state, &token, &csrf_token, "Erro interno. Tente novamente."),
    };

    // Atualizar senha e marcar token como usado atomicamente
    if let Err(e) = db::update_user_password(&state.pool, &user_id, &password_hash).await {
        tracing::error!("reset_password: update_user_password error: {e}");
        return render_error(&state, &token, &csrf_token, "Erro interno. Tente novamente.");
    }
    if let Err(e) = db::mark_reset_token_used(&state.pool, &token).await {
        tracing::error!("reset_password: mark_token_used error: {e}");
    }

    tracing::info!("reset_password: senha redefinida para user_id={}", user_id);
    Redirect::to("/login?reset=ok").into_response()
}

// ─── Auxiliar: valida token e retorna user_id ─────────────────────────────────

async fn validate_token(state: &AppState, token: &str) -> Result<String, &'static str> {
    if token.is_empty() || token.len() > 64 {
        return Err("Link inválido.");
    }
    match db::get_reset_token(&state.pool, token).await {
        Ok(Some((user_id, expires_at, used))) => {
            if used {
                return Err("Este link já foi utilizado. Solicite um novo.");
            }
            let expiry = chrono::DateTime::parse_from_rfc3339(&expires_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now() - Duration::seconds(1));
            if Utc::now() > expiry {
                return Err("Este link expirou. Solicite um novo.");
            }
            Ok(user_id)
        }
        Ok(None) => Err("Link inválido."),
        Err(e) => {
            tracing::error!("validate_token: DB error: {e}");
            Err("Erro interno. Tente novamente.")
        }
    }
}
