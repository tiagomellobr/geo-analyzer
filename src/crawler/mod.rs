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

/// Descobre todas as URLs do sitemap (incluindo sitemaps aninhados)
pub async fn discover_urls(client: &Client, site_url: &str) -> Result<Vec<String>> {
    let base = Url::parse(site_url)?;
    let sitemap_url = base.join("/sitemap.xml").map_err(|e| anyhow!(e))?;

    let mut all_urls = Vec::new();
    let mut sitemap_queue = vec![sitemap_url.to_string()];
    let mut visited_sitemaps = std::collections::HashSet::new();

    while let Some(sm_url) = sitemap_queue.pop() {
        if visited_sitemaps.contains(&sm_url) {
            continue;
        }
        visited_sitemaps.insert(sm_url.clone());

        match fetch_sitemap(client, &sm_url).await {
            Ok(result) => {
                all_urls.extend(result.page_urls);
                sitemap_queue.extend(result.sitemap_urls);
            }
            Err(e) => {
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

    if all_urls.is_empty() {
        // fallback: adicionar a própria URL raiz
        all_urls.push(site_url.to_string());
    }

    Ok(all_urls)
}

struct SitemapResult {
    page_urls: Vec<String>,
    sitemap_urls: Vec<String>,
}

async fn fetch_sitemap(client: &Client, url: &str) -> Result<SitemapResult> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let body = resp.text().await?;

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
    let resp = client.get(url).send().await?.error_for_status()?;
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
    Ok(page)
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
