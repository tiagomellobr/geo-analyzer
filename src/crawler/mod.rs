use anyhow::{anyhow, Result};
use futures::stream::{self, StreamExt};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::Client;
use scraper::{Html, Selector};
use url::Url;

const MAX_CONCURRENT: usize = 5;
const MAX_PAGES: usize = 200;

#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: String,
    pub html: String,
    pub text: String,
    pub title: Option<String>,
    pub meta_description: Option<String>,
    pub word_count: usize,
}

pub async fn build_client() -> Result<Client> {
    let client = Client::builder()
        .user_agent("GEOAnalyzer/1.0 (+https://github.com/geo-analyzer)")
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    Ok(client)
}

/// Extrai URLs de Sitemap declaradas no robots.txt
async fn discover_sitemaps_from_robots(client: &Client, base: &Url) -> Vec<String> {
    let robots_url = match base.join("/robots.txt") {
        Ok(u) => u.to_string(),
        Err(_) => return vec![],
    };

    let body = match client.get(&robots_url).send().await {
        Ok(resp) => match resp.text().await {
            Ok(t) => t,
            Err(_) => return vec![],
        },
        Err(_) => return vec![],
    };

    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("sitemap:") {
                let url = line[8..].trim().to_string();
                if url.starts_with("http://") || url.starts_with("https://") {
                    Some(url)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Paths alternativos de sitemap, em ordem de prioridade
const SITEMAP_FALLBACK_PATHS: &[&str] = &[
    "/sitemap.xml",
    "/sitemap_index.xml",
    "/sitemap-index.xml",
    "/sitemap/sitemap.xml",
    "/sitemaps/sitemap.xml",
];

/// Resultado da descoberta de URLs, incluindo possíveis avisos
pub struct DiscoveryResult {
    pub urls: Vec<String>,
    /// Aviso informativo quando nenhum sitemap foi encontrado,
    /// ou quando parte dos sitemaps falhou ao carregar.
    pub warning: Option<String>,
}

/// Descobre todas as URLs do sitemap (incluindo sitemaps aninhados)
pub async fn discover_urls(client: &Client, site_url: &str) -> Result<DiscoveryResult> {
    let base = Url::parse(site_url)?;

    // 1. Descobrir sitemaps via robots.txt
    let sitemaps_from_robots = discover_sitemaps_from_robots(client, &base).await;
    let from_robots = !sitemaps_from_robots.is_empty();

    // 2. Se robots.txt não declarou nenhum sitemap, tentar paths comuns
    let mut sitemap_queue: Vec<String> = if from_robots {
        sitemaps_from_robots
    } else {
        SITEMAP_FALLBACK_PATHS
            .iter()
            .filter_map(|path| base.join(path).ok().map(|u| u.to_string()))
            .collect()
    };

    let mut all_urls = Vec::new();
    let mut visited_sitemaps = std::collections::HashSet::new();
    let mut sitemap_found = false;
    let mut sitemap_errors = 0usize;

    while let Some(sm_url) = sitemap_queue.pop() {
        if visited_sitemaps.contains(&sm_url) {
            continue;
        }
        visited_sitemaps.insert(sm_url.clone());

        match fetch_sitemap(client, &sm_url).await {
            Ok(result) => {
                if !result.page_urls.is_empty() || !result.sitemap_urls.is_empty() {
                    sitemap_found = true;
                }
                all_urls.extend(result.page_urls);
                sitemap_queue.extend(result.sitemap_urls);
            }
            Err(e) => {
                sitemap_errors += 1;
                tracing::warn!("Falha ao buscar sitemap {}: {}", sm_url, e);
            }
        }

        if all_urls.len() >= MAX_PAGES {
            all_urls.truncate(MAX_PAGES);
            break;
        }
    }

    // Deduplicar
    all_urls.sort();
    all_urls.dedup();

    // Filtrar somente URLs do mesmo domínio
    let base_host = base.host_str().unwrap_or("").to_string();
    all_urls.retain(|u| {
        Url::parse(u)
            .map(|pu| pu.host_str().unwrap_or("") == base_host)
            .unwrap_or(false)
    });

    // Construir aviso conforme o caso
    let warning = if !sitemap_found && all_urls.is_empty() {
        let msg = if from_robots {
            format!(
                "Os sitemaps declarados no robots.txt falharam ao carregar ({} erro(s)). Apenas a URL raiz será analisada.",
                sitemap_errors
            )
        } else {
            "Nenhum sitemap.xml encontrado nos caminhos padrão. Apenas a URL raiz será analisada.".to_string()
        };
        all_urls.push(site_url.to_string());
        Some(msg)
    } else if sitemap_errors > 0 {
        Some(format!(
            "{} sitemap(s) falharam ao carregar e foram ignorados.",
            sitemap_errors
        ))
    } else {
        None
    };

    if let Some(ref w) = warning {
        tracing::warn!("{}", w);
    }

    Ok(DiscoveryResult {
        urls: all_urls,
        warning,
    })
}

struct SitemapResult {
    page_urls: Vec<String>,
    sitemap_urls: Vec<String>,
}

/// Extrai links para sitemaps XML a partir de uma página HTML
fn extract_sitemap_links_from_html(html: &str, base: &Url) -> Vec<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href]").unwrap();
    document
        .select(&selector)
        .filter_map(|el| el.value().attr("href"))
        .filter_map(|href| base.join(href).ok())
        .map(|u| u.to_string())
        .filter(|u| {
            let lower = u.to_ascii_lowercase();
            lower.ends_with(".xml") || lower.contains("sitemap")
        })
        .collect()
}

async fn fetch_sitemap(client: &Client, url: &str) -> Result<SitemapResult> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let final_url = resp.url().clone();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = resp.text().await?;

    // Se a resposta for HTML, tentar extrair links para sitemaps XML
    if content_type.contains("text/html") {
        let links = extract_sitemap_links_from_html(&body, &final_url);
        if links.is_empty() {
            return Err(anyhow!("Resposta HTML sem links para sitemap em {}", url));
        }
        tracing::info!(
            "Sitemap {} retornou HTML; {} link(s) encontrado(s)",
            url,
            links.len()
        );
        return Ok(SitemapResult {
            page_urls: vec![],
            sitemap_urls: links,
        });
    }

    let mut reader = Reader::from_str(&body);
    reader.config_mut().trim_text(true);

    let mut page_urls = Vec::new();
    let mut sitemap_urls = Vec::new();
    let mut in_loc = false;
    let mut in_sitemap = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.name();
                let local = name.local_name();
                match local.as_ref() {
                    b"loc" => in_loc = true,
                    b"sitemap" => in_sitemap = true,
                    b"url" => in_sitemap = false,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if in_loc {
                    let text = e.unescape().unwrap_or_default().trim().to_string();
                    if !text.is_empty() {
                        if in_sitemap {
                            sitemap_urls.push(text);
                        } else {
                            page_urls.push(text);
                        }
                    }
                    in_loc = false;
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.name().local_name();
                if local.as_ref() == b"sitemap" {
                    in_sitemap = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                tracing::warn!("Erro ao parsear sitemap XML: {}", e);
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(SitemapResult {
        page_urls,
        sitemap_urls,
    })
}

/// Baixa e parseia o HTML de cada URL
pub async fn crawl_pages(client: &Client, urls: Vec<String>) -> Vec<CrawledPage> {
    stream::iter(urls)
        .map(|url| {
            let client = client.clone();
            async move {
                match fetch_page(&client, &url).await {
                    Ok(page) => Some(page),
                    Err(e) => {
                        tracing::warn!("Erro ao baixar {}: {}", url, e);
                        None
                    }
                }
            }
        })
        .buffer_unordered(MAX_CONCURRENT)
        .filter_map(|x| async move { x })
        .collect()
        .await
}

pub async fn fetch_page(client: &Client, url: &str) -> Result<CrawledPage> {
    const MAX_RETRIES: u32 = 2;
    let mut attempt = 0u32;

    loop {
        let resp = client.get(url).send().await?;
        let status = resp.status();

        // 429 Too Many Requests — respeitar Retry-After ou aguardar 3s
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            if attempt < MAX_RETRIES {
                let wait_secs = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(3)
                    .min(30);
                attempt += 1;
                tracing::info!(
                    "Rate limited em {} — aguardando {}s (tentativa {}/{})",
                    url, wait_secs, attempt, MAX_RETRIES
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
                continue;
            }
            return Err(anyhow!(
                "Rate limiting persistente em {} após {} tentativas (HTTP 429)",
                url, attempt
            ));
        }

        // 5xx Server Error — uma retentativa após 2s
        if status.is_server_error() && attempt < 1 {
            attempt += 1;
            tracing::warn!(
                "Erro {} em {} — retentando em 2s (tentativa {})",
                status.as_u16(), url, attempt
            );
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        }

        // Outros erros HTTP
        let resp = resp.error_for_status()?;

        // Aceita somente HTML
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !content_type.contains("text/html") {
            return Err(anyhow!("Não é HTML: {}", content_type));
        }
        let html = resp.text().await?;
        let page = parse_html(url, &html);
        return Ok(page);
    }
}

pub fn parse_html(url: &str, html: &str) -> CrawledPage {
    let document = Html::parse_document(html);

    // Title
    let title = select_first_text(&document, "title");

    // Meta description
    let meta_description = {
        let sel = Selector::parse(r#"meta[name="description"]"#).unwrap();
        document
            .select(&sel)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.trim().to_string())
    };

    // Extrair texto visível: seleciona apenas elementos semânticos de conteúdo,
    // ignorando naturalmente script, style, nav, footer, header.
    let content_sel = Selector::parse(
        "p, h1, h2, h3, h4, h5, h6, li, td, th, blockquote, figcaption, article, section, main",
    )
    .unwrap();

    let text = document
        .select(&content_sel)
        .flat_map(|el| el.text())
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    let word_count = text.split_whitespace().count();

    CrawledPage {
        url: url.to_string(),
        html: html.to_string(),
        text,
        title,
        meta_description,
        word_count,
    }
}

fn select_first_text(document: &Html, selector: &str) -> Option<String> {
    let sel = Selector::parse(selector).ok()?;
    document
        .select(&sel)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}
