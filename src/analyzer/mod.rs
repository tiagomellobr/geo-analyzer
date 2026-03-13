pub mod llm;

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};

use crate::crawler::CrawledPage;
use crate::models::{GeoScores, Recommendation};

// ─── Regexes compiladas uma única vez ────────────────────────────────────────

static RE_STATISTICS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(\d+[\.,]?\d*\s*%|\d+\s+em\s+cada\s+\d+|\d+x\s|\bcrescimento\s+de\s+\d+|\b\d{2,}\s+mil\b|\bR\$\s*\d+|média\s+de\s+\d+|taxa\s+de\s+\d+)",
    )
    .unwrap()
});

static RE_QUOTATION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)([""][^""]{10,}[""]|[""][^"]{10,}["]|segundo\s+\w+[\s,]|de\s+acordo\s+com\s+\w+|conforme\s+\w+[\s,]|afirma\s+\w+|declarou\s+\w+)"#,
    )
    .unwrap()
});

static RE_HEDGING: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(talvez|pode\s+ser|acho\s+que|provavelmente|possivelmente|não\s+tenho\s+certeza|não\s+sei\s+ao\s+certo|quem\s+sabe|parece\s+que|ao\s+que\s+parece)\b")
        .unwrap()
});

static RE_AUTHORITATIVE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(comprova|demonstra|evidencia|confirma|prova|estudo\s+mostra|pesquisa\s+indica|dados\s+mostram|análise\s+revela|resultado\s+indica)\b")
        .unwrap()
});

// Domínios de autoridade para citações externas
const AUTHORITY_DOMAINS: &[&str] = &[
    ".gov", ".edu", "scholar.google", "pubmed", "scielo", "arxiv",
    "nature.com", "science.org", "ibge.gov.br", "who.int", "oecd.org",
    "worldbank.org", "un.org", "bbc.com", "reuters.com", "nih.gov",
];

// Termos técnicos genéricos que indicam especialização
const TECHNICAL_MARKERS: &[&str] = &[
    "metodologia", "framework", "pipeline", "benchmark", "algoritmo",
    "implementação", "arquitetura", "infraestrutura", "protocolo",
    "especificação", "norma", "regulamentação", "conformidade",
    "kpi", "roi", "sla", "api", "sdk", "saas", "cloud", "devops",
    "machine learning", "inteligência artificial", "otimização",
    "análise de dados", "estatística", "correlação", "variável",
    "hipótese", "metodologia científica", "peer review", "meta-análise",
];

/// Resultado completo da análise de uma página
pub struct AnalysisResult {
    pub scores: GeoScores,
    pub recommendations: Vec<Recommendation>,
    pub has_schema_markup: bool,
    pub has_og_tags: bool,
    /// Resultado fresco do LLM (apenas quando inferência foi executada nesta chamada,
    /// não quando veio de cache). Usado pelo worker para persistir no llm_cache.
    pub llm_analysis: Option<llm::LlmAnalysis>,
}

/// Analisa uma página calculando todos os scores GEO.
///
/// - `llm_client`: cliente Ollama para inferência. Ignorado se `llm_override` for Some.
/// - `llm_override`: resultado já em cache — pula a chamada ao LLM completamente.
///
/// Estratégia de concorrência: a chamada ao LLM (async, não-Send) é feita
/// ANTES de criar o `scraper::Html` (não-Send), evitando await com Html vivo.
pub async fn analyze_page(
    page: &CrawledPage,
    llm_client: Option<&llm::LlmClient>,
    llm_override: Option<llm::LlmAnalysis>,
) -> AnalysisResult {
    // Fase 1 — cache hit ou chamada LLM (async). Html ainda não foi criado.
    let (llm_result, is_fresh) = if let Some(cached) = llm_override {
        (Some(cached), false)
    } else {
        match llm_client {
            Some(client) => match client.analyze(&page.text).await {
                Ok(r) => {
                    tracing::debug!("LLM analysis OK para {}", page.url);
                    (Some(r), true)
                }
                Err(e) => {
                    tracing::warn!("LLM falhou para {}: {e}. Usando heurísticas.", page.url);
                    (None, false)
                }
            },
            None => (None, false),
        }
    };

    // Fase 2 — análise síncrona com HTML parsing (sem awaits)
    analyze_sync(page, llm_result, is_fresh)
}

fn analyze_sync(page: &CrawledPage, llm_result: Option<llm::LlmAnalysis>, is_fresh: bool) -> AnalysisResult {
    let document = Html::parse_document(&page.html);
    let text_lc = page.text.to_lowercase();

    // Critérios objetivos — sempre via heurística
    let cite_sources = score_cite_sources(&document, &page.html);
    let quotation_addition = score_quotation_addition(&page.text);
    let statistics_addition = score_statistics_addition(&page.text);
    let content_structure = score_content_structure(&document);
    let (metadata_quality, has_og_tags) =
        score_metadata_quality(&document, &page.title, &page.meta_description);
    let (schema_markup, has_schema_markup) = score_schema_markup(&page.html);
    let content_depth = score_content_depth(&page.text, page.word_count);

    // Critérios subjetivos — LLM quando disponível, heurística como fallback
    let fluency = llm_result
        .as_ref()
        .map(|r| r.fluency)
        .unwrap_or_else(|| score_fluency(&page.text));
    let authoritative_tone = llm_result
        .as_ref()
        .map(|r| r.authoritative_tone)
        .unwrap_or_else(|| score_authoritative_tone(&page.text, &text_lc));
    let technical_terms = llm_result
        .as_ref()
        .map(|r| r.technical_terms)
        .unwrap_or_else(|| score_technical_terms(&text_lc));
    let easy_to_understand = llm_result
        .as_ref()
        .map(|r| r.easy_to_understand)
        .unwrap_or_else(|| score_easy_to_understand(&page.text, &document));

    let scores = GeoScores {
        cite_sources,
        quotation_addition,
        statistics_addition,
        fluency,
        authoritative_tone,
        technical_terms,
        easy_to_understand,
        content_structure,
        metadata_quality,
        schema_markup,
        content_depth,
    };

    let recommendations =
        build_recommendations(&scores, has_og_tags, has_schema_markup, llm_result.as_ref());

    // Expor resultado fresco do LLM para que o worker possa persistir no cache
    let llm_analysis = if is_fresh { llm_result } else { None };

    AnalysisResult {
        scores,
        recommendations,
        has_schema_markup,
        has_og_tags,
        llm_analysis,
    }
}

// ─── Critérios individuais ────────────────────────────────────────────────────

fn score_cite_sources(document: &Html, _html: &str) -> f64 {
    let sel = Selector::parse("a[href]").unwrap();
    let links: Vec<_> = document.select(&sel).collect();

    if links.is_empty() {
        return 0.0;
    }

    let authority_count = links.iter().filter(|el| {
        el.value().attr("href").map(|href| {
            let href_lc = href.to_lowercase();
            // Links externos
            (href.starts_with("http://") || href.starts_with("https://"))
                && AUTHORITY_DOMAINS.iter().any(|d| href_lc.contains(d))
        }).unwrap_or(false)
    }).count();

    let external_count = links.iter().filter(|el| {
        el.value().attr("href").map(|href| {
            href.starts_with("http://") || href.starts_with("https://")
        }).unwrap_or(false)
    }).count();

    if authority_count >= 3 {
        1.0
    } else if authority_count == 2 {
        0.8
    } else if authority_count == 1 {
        0.5
    } else if external_count >= 2 {
        0.3
    } else if external_count == 1 {
        0.15
    } else {
        0.0
    }
}

fn score_quotation_addition(text: &str) -> f64 {
    let count = RE_QUOTATION.find_iter(text).count();
    if count >= 3 {
        1.0
    } else if count == 2 {
        0.75
    } else if count == 1 {
        0.4
    } else {
        0.0
    }
}

fn score_statistics_addition(text: &str) -> f64 {
    let count = RE_STATISTICS.find_iter(text).count();
    if count >= 4 {
        1.0
    } else if count == 3 {
        0.8
    } else if count == 2 {
        0.6
    } else if count == 1 {
        0.35
    } else {
        0.0
    }
}

fn score_fluency(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }

    let sentences: Vec<&str> = text
        .split(|c| c == '.' || c == '!' || c == '?')
        .filter(|s| s.split_whitespace().count() >= 3)
        .collect();

    if sentences.is_empty() {
        return 0.3;
    }

    let avg_words_per_sentence = sentences
        .iter()
        .map(|s| s.split_whitespace().count())
        .sum::<usize>() as f64
        / sentences.len() as f64;

    // Ideal: 15–25 palavras por frase
    let sentence_score = if avg_words_per_sentence >= 10.0 && avg_words_per_sentence <= 25.0 {
        1.0
    } else if avg_words_per_sentence < 10.0 {
        avg_words_per_sentence / 10.0
    } else {
        // frases muito longas penalizam
        (50.0 - avg_words_per_sentence).max(0.0) / 25.0
    };

    // Penalizar textos muito curtos
    let word_count = text.split_whitespace().count();
    let length_score = if word_count >= 300 {
        1.0
    } else {
        word_count as f64 / 300.0
    };

    (sentence_score * 0.7 + length_score * 0.3).min(1.0)
}

fn score_authoritative_tone(text: &str, text_lc: &str) -> f64 {
    let word_count = text.split_whitespace().count().max(1);

    // Penalizar linguagem vacilante (hedging)
    let hedge_count = RE_HEDGING.find_iter(text_lc).count();
    let hedge_density = hedge_count as f64 / (word_count as f64 / 100.0);

    // Recompensar afirmações autoritativas
    let auth_count = RE_AUTHORITATIVE.find_iter(text_lc).count();

    let base = if auth_count >= 2 { 0.8 } else if auth_count == 1 { 0.5 } else { 0.3 };
    let penalty = (hedge_density * 0.1).min(0.5);

    (base - penalty).max(0.0).min(1.0)
}

fn score_technical_terms(text_lc: &str) -> f64 {
    let count = TECHNICAL_MARKERS
        .iter()
        .filter(|&&term| text_lc.contains(term))
        .count();

    if count >= 5 {
        1.0
    } else if count >= 3 {
        0.7
    } else if count >= 1 {
        0.4
    } else {
        0.0
    }
}

fn score_easy_to_understand(text: &str, document: &Html) -> f64 {
    // Listas (ul/ol) melhoram compreensão
    let list_sel = Selector::parse("ul, ol").unwrap();
    let list_count = document.select(&list_sel).count();

    // Subtítulos
    let heading_sel = Selector::parse("h2, h3, h4").unwrap();
    let heading_count = document.select(&heading_sel).count();

    let word_count = text.split_whitespace().count();

    let list_score = if list_count >= 2 { 1.0 } else if list_count == 1 { 0.5 } else { 0.0 };
    let heading_score = if heading_count >= 3 {
        1.0
    } else if heading_count >= 1 {
        0.5
    } else {
        0.0
    };

    // Comprimento razoável: 300–2000 palavras é ideal
    let length_score = if word_count >= 300 && word_count <= 2000 {
        1.0
    } else if word_count < 300 {
        word_count as f64 / 300.0
    } else {
        0.8 // muito longo, mas não tão ruim
    };

    (list_score * 0.3 + heading_score * 0.4 + length_score * 0.3).min(1.0)
}

fn score_content_structure(document: &Html) -> f64 {
    let h1_sel = Selector::parse("h1").unwrap();
    let h2_sel = Selector::parse("h2").unwrap();
    let h3_sel = Selector::parse("h3").unwrap();
    let list_sel = Selector::parse("ul li, ol li").unwrap();

    let h1_count = document.select(&h1_sel).count();
    let h2_count = document.select(&h2_sel).count();
    let h3_count = document.select(&h3_sel).count();
    let list_items = document.select(&list_sel).count();

    let h1_score: f64 = if h1_count == 1 { 1.0 } else if h1_count > 1 { 0.5 } else { 0.0 };
    let h2_score: f64 = if h2_count >= 2 { 1.0 } else if h2_count == 1 { 0.5 } else { 0.0 };
    let h3_score: f64 = if h3_count >= 1 { 0.5 } else { 0.0 };
    let list_score: f64 = if list_items >= 3 { 1.0 } else if list_items >= 1 { 0.5 } else { 0.0 };

    (h1_score * 0.3 + h2_score * 0.3 + h3_score * 0.2 + list_score * 0.2).min(1.0_f64)
}

fn score_metadata_quality(
    document: &Html,
    title: &Option<String>,
    meta_description: &Option<String>,
) -> (f64, bool) {
    let title_score: f64 = match title {
        Some(t) => {
            let len = t.len();
            if len >= 30 && len <= 70 {
                1.0
            } else if len > 0 {
                0.5
            } else {
                0.0
            }
        }
        None => 0.0,
    };

    let desc_score = match meta_description {
        Some(d) => {
            let len = d.len();
            if len >= 100 && len <= 160 {
                1.0
            } else if len > 0 {
                0.5
            } else {
                0.0
            }
        }
        None => 0.0,
    };

    // Open Graph
    let og_sel = Selector::parse(r#"meta[property^="og:"]"#).unwrap();
    let og_count = document.select(&og_sel).count();
    let has_og = og_count >= 2;
    let og_score: f64 = if has_og { 1.0 } else if og_count == 1 { 0.5 } else { 0.0 };

    let score = (title_score * 0.4 + desc_score * 0.4 + og_score * 0.2).min(1.0_f64);
    (score, has_og)
}

fn score_schema_markup(html: &str) -> (f64, bool) {
    let has_json_ld = html.contains(r#"application/ld+json"#);
    let has_itemtype = html.contains("itemtype=") || html.contains("itemscope");

    if has_json_ld {
        (1.0, true)
    } else if has_itemtype {
        (0.6, true)
    } else {
        (0.0, false)
    }
}

fn score_content_depth(text: &str, word_count: usize) -> f64 {
    // Mínimo de 600 palavras para conteúdo profundo
    let word_score = if word_count >= 1200 {
        1.0
    } else if word_count >= 600 {
        0.7
    } else if word_count >= 300 {
        0.4
    } else {
        word_count as f64 / 300.0
    };

    // Verificar se responde perguntas (5W1H)
    let text_lc = text.to_lowercase();
    let five_w_keywords = ["como", "por que", "quando", "onde", "quem", "o que", "qual"];
    let question_count = five_w_keywords
        .iter()
        .filter(|&&kw| text_lc.contains(kw))
        .count();
    let question_score = (question_count as f64 / 4.0).min(1.0);

    (word_score * 0.6 + question_score * 0.4).min(1.0)
}

// ─── Recomendações ────────────────────────────────────────────────────────────

fn build_recommendations(
    scores: &GeoScores,
    has_og_tags: bool,
    has_schema_markup: bool,
    llm: Option<&llm::LlmAnalysis>,
) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    if scores.cite_sources < 0.5 {
        recs.push(Recommendation {
            criterion: "Citações de Fontes".to_string(),
            message: "Adicione referências a fontes externas confiáveis, como estudos, dados governamentais ou publicações reconhecidas.".to_string(),
            impact: "Alta (+25%)".to_string(),
        });
    }
    if scores.quotation_addition < 0.5 {
        recs.push(Recommendation {
            criterion: "Citações Diretas".to_string(),
            message: "Inclua citações diretas de especialistas, pesquisas ou publicações, com atribuição clara da fonte.".to_string(),
            impact: "Muito Alta (+41%)".to_string(),
        });
    }
    if scores.statistics_addition < 0.5 {
        recs.push(Recommendation {
            criterion: "Dados e Estatísticas".to_string(),
            message: "Adicione dados quantitativos, percentuais e estatísticas para embasar suas afirmações.".to_string(),
            impact: "Alta (+30%)".to_string(),
        });
    }
    if scores.fluency < 0.5 {
        let msg = llm
            .and_then(|r| r.fluency_recommendation.as_deref())
            .unwrap_or("Revise a legibilidade do texto. Prefira frases mais curtas, parágrafos menores e linguagem clara.")
            .to_string();
        recs.push(Recommendation {
            criterion: "Fluência e Legibilidade".to_string(),
            message: msg,
            impact: "Alta (+27%)".to_string(),
        });
    }
    if scores.authoritative_tone < 0.5 {
        let msg = llm
            .and_then(|r| r.authoritative_tone_recommendation.as_deref())
            .unwrap_or("Use tom autoritativo. Evite linguagem vaga ou incerta. Faça afirmações claras baseadas em evidências.")
            .to_string();
        recs.push(Recommendation {
            criterion: "Tom Autoritativo".to_string(),
            message: msg,
            impact: "Média (+10.5%)".to_string(),
        });
    }
    if scores.technical_terms < 0.4 {
        let msg = llm
            .and_then(|r| r.technical_terms_recommendation.as_deref())
            .unwrap_or("Incorpore terminologia técnica relevante ao seu domínio para demonstrar especialização.")
            .to_string();
        recs.push(Recommendation {
            criterion: "Termos Técnicos".to_string(),
            message: msg,
            impact: "Média (+17%)".to_string(),
        });
    }
    if scores.easy_to_understand < 0.5 {
        let msg = llm
            .and_then(|r| r.easy_to_understand_recommendation.as_deref())
            .unwrap_or("Simplifique termos complexos, use exemplos concretos e estruture o conteúdo com subtítulos e listas.")
            .to_string();
        recs.push(Recommendation {
            criterion: "Clareza e Acessibilidade".to_string(),
            message: msg,
            impact: "Média (+14%)".to_string(),
        });
    }
    if scores.content_structure < 0.5 {
        recs.push(Recommendation {
            criterion: "Estrutura de Conteúdo".to_string(),
            message: "Use uma hierarquia clara de títulos (H1, H2, H3) e organize informações em listas quando possível.".to_string(),
            impact: "Baixa".to_string(),
        });
    }
    if scores.metadata_quality < 0.5 {
        recs.push(Recommendation {
            criterion: "Metadados".to_string(),
            message: "Adicione um título descritivo (50–60 chars) e meta description informativa (150–160 chars).".to_string(),
            impact: "Baixa".to_string(),
        });
    }
    if !has_og_tags {
        recs.push(Recommendation {
            criterion: "Open Graph Tags".to_string(),
            message: "Adicione tags Open Graph (og:title, og:description, og:image) para melhorar o compartilhamento social.".to_string(),
            impact: "Baixa".to_string(),
        });
    }
    if !has_schema_markup {
        recs.push(Recommendation {
            criterion: "Schema Markup".to_string(),
            message: "Adicione JSON-LD com schema.org (Article, FAQPage, HowTo) para ajudar GEs a entender o contexto do conteúdo.".to_string(),
            impact: "Baixa".to_string(),
        });
    }
    if scores.content_depth < 0.5 {
        recs.push(Recommendation {
            criterion: "Profundidade do Conteúdo".to_string(),
            message: "Expanda o conteúdo para pelo menos 600 palavras e responda às perguntas fundamentais do seu público (Como? Por quê? Quando?).".to_string(),
            impact: "Baixa".to_string(),
        });
    }

    recs
}
