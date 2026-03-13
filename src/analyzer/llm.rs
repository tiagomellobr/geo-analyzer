use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Cliente para o Ollama local
pub struct LlmClient {
    client: Client,
    pub host: String,
    pub model: String,
}

/// Resultado da análise semântica para os 4 critérios subjetivos
#[derive(Debug, Clone, Default)]
pub struct LlmAnalysis {
    pub fluency: f64,
    pub authoritative_tone: f64,
    pub technical_terms: f64,
    pub easy_to_understand: f64,
    pub fluency_recommendation: Option<String>,
    pub authoritative_tone_recommendation: Option<String>,
    pub technical_terms_recommendation: Option<String>,
    pub easy_to_understand_recommendation: Option<String>,
}

// ─── Tipos internos de (de)serialização ──────────────────────────────────────

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    format: &'a str,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

#[derive(Deserialize)]
struct CriterionOutput {
    #[serde(default)]
    score: f64,
    #[serde(default)]
    recommendation: String,
}

#[derive(Deserialize, Default)]
struct LlmJsonOutput {
    fluency: Option<CriterionOutput>,
    authoritative_tone: Option<CriterionOutput>,
    technical_terms: Option<CriterionOutput>,
    easy_to_understand: Option<CriterionOutput>,
}

// ─── Implementação ────────────────────────────────────────────────────────────

impl LlmClient {
    pub fn new() -> Self {
        let host = std::env::var("OLLAMA_HOST")
            .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
        let model = std::env::var("OLLAMA_MODEL")
            .unwrap_or_else(|_| "gemma3:1b".to_string());

        let client = Client::builder()
            .timeout(Duration::from_secs(
                std::env::var("OLLAMA_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30),
            ))
            .build()
            .expect("Falha ao criar cliente HTTP para Ollama");

        Self { client, host, model }
    }

    /// Retorna `true` se o Ollama estiver acessível (verificação rápida).
    pub async fn is_available(&self) -> bool {
        self.client
            .get(format!("{}/api/tags", self.host))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Garante que o modelo está disponível no Ollama, puxando-o se necessário.
    ///
    /// Usa um cliente HTTP sem timeout pois o download pode demorar vários minutos.
    pub async fn ensure_model_pulled(&self) -> Result<()> {
        #[derive(Serialize)]
        struct PullRequest<'a> {
            name: &'a str,
            stream: bool,
        }

        // Cliente sem timeout: pull pode demorar minutos
        let pull_client = Client::builder()
            .build()
            .expect("Falha ao criar cliente HTTP para pull");

        tracing::info!(
            "Verificando/baixando modelo '{}' via Ollama (pode demorar)...",
            self.model
        );

        let resp = pull_client
            .post(format!("{}/api/pull", self.host))
            .json(&PullRequest { name: &self.model, stream: false })
            .send()
            .await
            .map_err(|e| anyhow!("Falha ao contatar Ollama para pull: {e}"))?;

        if resp.status().is_success() {
            tracing::info!("Modelo '{}' pronto.", self.model);
            Ok(())
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(anyhow!(
                "Falha ao puxar modelo '{}': HTTP {} — {}",
                self.model,
                status,
                &body[..body.len().min(200)]
            ))
        }
    }

    /// Analisa os 4 critérios subjetivos de GEO via LLM.
    ///
    /// O texto é truncado em ~1 500 caracteres (~300 tokens) para manter
    /// o tempo de inferência baixo. Em caso de falha retorna `Err` e o
    /// chamador deve aplicar o fallback heurístico.
    pub async fn analyze(&self, text: &str) -> Result<LlmAnalysis> {
        // Truncar para controlar tamanho do prompt
        let excerpt: String = text.chars().take(1500).collect();

        let prompt = build_prompt(&excerpt);

        let req = OllamaRequest {
            model: &self.model,
            prompt: &prompt,
            stream: false,
            format: "json",
        };

        let resp = self
            .client
            .post(format!("{}/api/generate", self.host))
            .json(&req)
            .send()
            .await
            .map_err(|e| anyhow!("Requisição ao Ollama falhou: {e}"))?;

        if !resp.status().is_success() {
            return Err(anyhow!(
                "Ollama retornou status HTTP {}",
                resp.status()
            ));
        }

        let body: OllamaResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("Falha ao deserializar resposta do Ollama: {e}"))?;

        // Tentar parsear o JSON — se o modelo não retornar JSON válido,
        // sanitizamos tentando extrair o primeiro objeto
        let parsed: LlmJsonOutput = parse_llm_output(&body.response)
            .map_err(|e| anyhow!("Falha ao parsear JSON do LLM: {e}\nResposta bruta: {}", &body.response[..body.response.len().min(200)]))?;

        Ok(extract_analysis(parsed))
    }
}

// ─── Helpers privados ─────────────────────────────────────────────────────────

fn build_prompt(excerpt: &str) -> String {
    format!(
        r#"Você é um especialista em GEO (Generative Engine Optimization). Analise o trecho de conteúdo abaixo e avalie 4 critérios. Retorne APENAS um objeto JSON válido, sem texto adicional antes ou depois.

Critérios (score de 0.0 a 1.0 e recomendação curta em português):
- fluency: o texto tem boa fluidez, frases bem construídas e coerentes?
- authoritative_tone: o tom é confiante e direto, sem linguagem vaga ("talvez", "acho que", "possivelmente")?
- technical_terms: o texto usa terminologia técnica adequada ao domínio?
- easy_to_understand: o conteúdo é claro e acessível para diferentes perfis de leitores?

Responda SOMENTE com este JSON (preencha os valores):
{{"fluency":{{"score":0.0,"recommendation":""}},"authoritative_tone":{{"score":0.0,"recommendation":""}},"technical_terms":{{"score":0.0,"recommendation":""}},"easy_to_understand":{{"score":0.0,"recommendation":""}}}}

<content>
{excerpt}
</content>"#
    )
}

fn parse_llm_output(raw: &str) -> Result<LlmJsonOutput> {
    // Tentativa direta
    if let Ok(parsed) = serde_json::from_str::<LlmJsonOutput>(raw) {
        return Ok(parsed);
    }

    // Tentar extrair primeiro {...} do texto (caso o modelo adicione prefácio)
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw.rfind('}') {
            let slice = &raw[start..=end];
            if let Ok(parsed) = serde_json::from_str::<LlmJsonOutput>(slice) {
                return Ok(parsed);
            }
        }
    }

    Err(anyhow!("Nenhum JSON válido encontrado na resposta"))
}

fn extract_analysis(parsed: LlmJsonOutput) -> LlmAnalysis {
    fn process(opt: Option<CriterionOutput>) -> (f64, Option<String>) {
        match opt {
            Some(c) => {
                let score = c.score.clamp(0.0, 1.0);
                let rec = if c.recommendation.trim().is_empty() {
                    None
                } else {
                    Some(c.recommendation.trim().to_string())
                };
                (score, rec)
            }
            None => (0.5, None),
        }
    }

    let (fluency, fluency_rec) = process(parsed.fluency);
    let (authoritative_tone, auth_rec) = process(parsed.authoritative_tone);
    let (technical_terms, tech_rec) = process(parsed.technical_terms);
    let (easy_to_understand, easy_rec) = process(parsed.easy_to_understand);

    LlmAnalysis {
        fluency,
        authoritative_tone,
        technical_terms,
        easy_to_understand,
        fluency_recommendation: fluency_rec,
        authoritative_tone_recommendation: auth_rec,
        technical_terms_recommendation: tech_rec,
        easy_to_understand_recommendation: easy_rec,
    }
}
