use printpdf::*;

use crate::models::{Job, Page};

// ─── Text helpers ─────────────────────────────────────────────────────────────

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'A',
            'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
            'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'O',
            'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
            'Ç' => 'C',
            'Ñ' => 'N',
            c if c.is_ascii() => c,
            _ => '_',
        })
        .collect()
}

fn trunc(s: &str, max: usize) -> String {
    let s = sanitize(s);
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        chars.iter().collect()
    } else {
        let cut = max.saturating_sub(3);
        let truncated: String = chars[..cut].iter().collect();
        format!("{}...", truncated)
    }
}

fn word_wrap(text: &str, max_chars: usize) -> Vec<String> {
    let text = sanitize(text);
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= max_chars {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

// ─── Color / badge helpers ───────────────────────────────────────────────────

fn score_color(score_pct: f64) -> Color {
    if score_pct >= 80.0 {
        Color::Rgb(Rgb { r: 0.11, g: 0.53, b: 0.11, icc_profile: None })
    } else if score_pct >= 50.0 {
        Color::Rgb(Rgb { r: 0.70, g: 0.45, b: 0.00, icc_profile: None })
    } else {
        Color::Rgb(Rgb { r: 0.80, g: 0.10, b: 0.10, icc_profile: None })
    }
}

fn badge_label(score_pct: f64) -> &'static str {
    if score_pct >= 80.0 {
        "Excelente"
    } else if score_pct >= 50.0 {
        "Moderado"
    } else {
        "Critico"
    }
}

// ─── Drawing helpers (printpdf 0.6: Mm=f32, Polygon API) ────────────────────

fn mm(v: f32) -> Mm {
    Mm(v)
}

/// Filled rectangle using printpdf 0.6 Polygon API.
fn filled_rect(layer: &PdfLayerReference, x1: f32, y1: f32, x2: f32, y2: f32) {
    layer.add_polygon(Polygon {
        rings: vec![vec![
            (Point::new(Mm(x1), Mm(y1)), false),
            (Point::new(Mm(x2), Mm(y1)), false),
            (Point::new(Mm(x2), Mm(y2)), false),
            (Point::new(Mm(x1), Mm(y2)), false),
        ]],
        mode: PolygonMode::Fill,
        winding_order: WindingOrder::NonZero,
    });
}

fn dark_text(layer: &PdfLayerReference) {
    layer.set_fill_color(Color::Rgb(Rgb { r: 0.10, g: 0.10, b: 0.10, icc_profile: None }));
}

fn gray_text(layer: &PdfLayerReference) {
    layer.set_fill_color(Color::Rgb(Rgb { r: 0.50, g: 0.50, b: 0.50, icc_profile: None }));
}

fn white_text(layer: &PdfLayerReference) {
    layer.set_fill_color(Color::Rgb(Rgb { r: 1.00, g: 1.00, b: 1.00, icc_profile: None }));
}

// ─── Helpers de layout ────────────────────────────────────────────────────────

const PAGE_W: f32 = 210.0;
const PAGE_H: f32 = 297.0;
const MARGIN: f32 = 14.0;

/// Barra de progresso horizontal com bg cinza + fill colorido + texto à direita.
fn progress_bar(
    layer: &PdfLayerReference,
    x: f32, y: f32, bar_w: f32, bar_h: f32,
    pct: f64,
) {
    // bg
    layer.set_fill_color(Color::Rgb(Rgb { r: 0.85, g: 0.85, b: 0.87, icc_profile: None }));
    filled_rect(layer, x, y, x + bar_w, y + bar_h);
    // fill
    let fill = (pct.clamp(0.0, 100.0) / 100.0 * bar_w as f64) as f32;
    if fill > 0.0 {
        layer.set_fill_color(score_color(pct));
        filled_rect(layer, x, y, x + fill, y + bar_h);
    }
}

/// Retângulo com borda (apenas borda, sem fill).
fn stroke_rect(layer: &PdfLayerReference, x1: f32, y1: f32, x2: f32, y2: f32) {
    layer.add_polygon(Polygon {
        rings: vec![vec![
            (Point::new(Mm(x1), Mm(y1)), false),
            (Point::new(Mm(x2), Mm(y1)), false),
            (Point::new(Mm(x2), Mm(y2)), false),
            (Point::new(Mm(x1), Mm(y2)), false),
        ]],
        mode: PolygonMode::Stroke,
        winding_order: WindingOrder::NonZero,
    });
}

/// Footer padrão para todas as páginas.
fn draw_footer(
    layer: &PdfLayerReference,
    reg: &IndirectFontRef,
    current: usize,
    total: usize,
) {
    // linha divisória
    layer.set_fill_color(Color::Rgb(Rgb { r: 0.80, g: 0.80, b: 0.85, icc_profile: None }));
    filled_rect(layer, MARGIN, 13.5, PAGE_W - MARGIN, 14.0);

    gray_text(layer);
    layer.use_text(
        &format!("GEO Analyzer  |  Pagina {}/{}", current, total),
        7.5_f32, mm(MARGIN), mm(9.0), reg,
    );
    layer.use_text(
        "geo-analyzer.app",
        7.5_f32, mm(PAGE_W - MARGIN - 28.0), mm(9.0), reg,
    );
}

// ─── Main PDF generation ─────────────────────────────────────────────────────

/// Gera os bytes do relatório PDF.
/// `include_detail`: incluir seção por página (uma folha por página analisada).
pub fn generate_pdf_bytes(
    job: &Job,
    pages: &[Page],
    include_detail: bool,
) -> anyhow::Result<Vec<u8>> {
    let avg_score_pct = if pages.is_empty() {
        0.0_f64
    } else {
        pages.iter().map(|p| p.geo_score * 100.0).sum::<f64>() / pages.len() as f64
    };
    let critical_count = pages.iter().filter(|p| p.geo_score * 100.0 < 50.0).count();
    let excellent_count = pages.iter().filter(|p| p.geo_score * 100.0 >= 80.0).count();
    let moderate_count = pages.len() - critical_count - excellent_count;

    let mut sorted: Vec<&Page> = pages.iter().collect();
    sorted.sort_by(|a, b| {
        b.geo_score
            .partial_cmp(&a.geo_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (doc, pg1, ly1) =
        PdfDocument::new("GEO Analyzer Report", Mm(PAGE_W), Mm(PAGE_H), "Main");
    let bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;
    let reg = doc.add_builtin_font(BuiltinFont::Helvetica)?;

    // ── Contagem de páginas ──────────────────────────────────────────────────
    const ROWS_PER_PAGE: usize = 25;
    let table_pages_count = ((sorted.len() + ROWS_PER_PAGE - 1) / ROWS_PER_PAGE).max(1);
    let detail_pages_count = if include_detail { sorted.len() } else { 0 };
    let total_pdf_pages = 1 + table_pages_count + detail_pages_count;

    // ── Cover page ───────────────────────────────────────────────────────────
    let cover = doc.get_page(pg1).get_layer(ly1);

    // Faixa azul escura no topo (altura 100 mm)
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.102, g: 0.196, b: 0.392, icc_profile: None }));
    filled_rect(&cover, 0.0, 197.0, PAGE_W, PAGE_H);

    // Acento lateral esquerdo colorido
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.22, g: 0.51, b: 0.94, icc_profile: None }));
    filled_rect(&cover, 0.0, 197.0, 5.0, PAGE_H);

    white_text(&cover);
    cover.use_text("GEO Analyzer", 34.0_f32, mm(14.0), mm(268.0), &bold);
    cover.use_text("Relatorio de Analise GEO", 16.0_f32, mm(14.0), mm(256.0), &reg);

    // URL e data — cor mais clara
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.75, g: 0.82, b: 0.96, icc_profile: None }));
    cover.use_text(&trunc(&job.site_url, 62), 10.5_f32, mm(14.0), mm(244.0), &reg);
    let date = if job.created_at.len() >= 10 { &job.created_at[..10] } else { &job.created_at };
    cover.use_text(&format!("Gerado em: {}", date), 9.5_f32, mm(14.0), mm(235.0), &reg);

    // ── Cards de métricas (3 cartões lado a lado) ────────────────────────────
    // Card 1 – Score Médio
    let c1x = MARGIN;
    let card_y_top = 175.0_f32;
    let card_y_bot = 120.0_f32;
    let card_w = 54.0_f32;
    let gap = 8.0_f32;

    // sombra simulada
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.82, g: 0.82, b: 0.86, icc_profile: None }));
    filled_rect(&cover, c1x + 1.5, card_y_bot - 1.5, c1x + card_w + 1.5, card_y_top - 1.5);

    cover.set_fill_color(Color::Rgb(Rgb { r: 0.97, g: 0.97, b: 0.99, icc_profile: None }));
    filled_rect(&cover, c1x, card_y_bot, c1x + card_w, card_y_top);

    // borda superior colorida
    cover.set_fill_color(score_color(avg_score_pct));
    filled_rect(&cover, c1x, card_y_top - 2.0, c1x + card_w, card_y_top);

    gray_text(&cover);
    cover.use_text("Score Medio", 8.0_f32, mm(c1x + 3.0), mm(card_y_top - 9.0), &reg);
    cover.set_fill_color(score_color(avg_score_pct));
    cover.use_text(
        &format!("{:.0}", avg_score_pct),
        40.0_f32, mm(c1x + 8.0), mm(card_y_bot + 22.0), &bold,
    );
    cover.set_fill_color(score_color(avg_score_pct));
    cover.use_text(badge_label(avg_score_pct), 10.0_f32, mm(c1x + 5.0), mm(card_y_bot + 10.0), &bold);

    // Card 2 – Páginas
    let c2x = c1x + card_w + gap;
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.82, g: 0.82, b: 0.86, icc_profile: None }));
    filled_rect(&cover, c2x + 1.5, card_y_bot - 1.5, c2x + card_w + 1.5, card_y_top - 1.5);
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.97, g: 0.97, b: 0.99, icc_profile: None }));
    filled_rect(&cover, c2x, card_y_bot, c2x + card_w, card_y_top);
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.22, g: 0.51, b: 0.94, icc_profile: None }));
    filled_rect(&cover, c2x, card_y_top - 2.0, c2x + card_w, card_y_top);

    gray_text(&cover);
    cover.use_text("Total de Paginas", 8.0_f32, mm(c2x + 3.0), mm(card_y_top - 9.0), &reg);
    dark_text(&cover);
    cover.use_text(
        &format!("{}", pages.len()),
        40.0_f32, mm(c2x + 10.0), mm(card_y_bot + 22.0), &bold,
    );
    gray_text(&cover);
    cover.use_text("paginas analisadas", 8.0_f32, mm(c2x + 3.0), mm(card_y_bot + 10.0), &reg);

    // Card 3 – Distribuição
    let c3x = c2x + card_w + gap;
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.82, g: 0.82, b: 0.86, icc_profile: None }));
    filled_rect(&cover, c3x + 1.5, card_y_bot - 1.5, c3x + card_w + 1.5, card_y_top - 1.5);
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.97, g: 0.97, b: 0.99, icc_profile: None }));
    filled_rect(&cover, c3x, card_y_bot, c3x + card_w, card_y_top);
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.11, g: 0.53, b: 0.11, icc_profile: None }));
    filled_rect(&cover, c3x, card_y_top - 2.0, c3x + card_w, card_y_top);

    gray_text(&cover);
    cover.use_text("Distribuicao", 8.0_f32, mm(c3x + 3.0), mm(card_y_top - 9.0), &reg);
    // mini legenda
    let dist_items: &[(&str, Color, usize)] = &[
        ("Excelente", Color::Rgb(Rgb { r: 0.11, g: 0.53, b: 0.11, icc_profile: None }), excellent_count),
        ("Moderado",  Color::Rgb(Rgb { r: 0.70, g: 0.45, b: 0.00, icc_profile: None }), moderate_count),
        ("Critico",   Color::Rgb(Rgb { r: 0.80, g: 0.10, b: 0.10, icc_profile: None }), critical_count),
    ];
    let mut dy = card_y_top - 18.0;
    for (label, color, count) in dist_items {
        cover.set_fill_color(color.clone());
        filled_rect(&cover, c3x + 4.0, dy - 1.0, c3x + 8.0, dy + 3.0);
        dark_text(&cover);
        cover.use_text(
            &format!("{}: {}", label, count),
            8.0_f32, mm(c3x + 10.0), mm(dy), &reg,
        );
        dy -= 10.0;
    }

    // ── "O que avaliamos" ────────────────────────────────────────────────────
    // Título da seção
    dark_text(&cover);
    cover.use_text("O que avaliamos", 10.5_f32, mm(MARGIN), mm(114.0), &bold);
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.40, g: 0.40, b: 0.55, icc_profile: None }));
    cover.use_text("(baseado no paper KDD '24)", 8.0_f32, mm(MARGIN + 47.0), mm(114.0), &reg);

    // Linha decorativa abaixo do título
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.22, g: 0.51, b: 0.94, icc_profile: None }));
    filled_rect(&cover, MARGIN, 110.5, PAGE_W - MARGIN, 111.5);

    // Grade 2 colunas × 5 linhas
    let kdd_items: &[(&str, &str)] = &[
        ("Citacoes de Fontes",      "+25%"),
        ("Citacoes Diretas",        "+41%"),
        ("Dados e Estatisticas",    "+30%"),
        ("Fluencia e Legibilidade", "+27%"),
        ("Tom Autoritativo",        "+10%"),
        ("Termos Tecnicos",         "+17%"),
        ("Clareza e Acessibilidade","+14%"),
        ("Estrutura de Conteudo",   "---"),
        ("Metadados e Schema",      "---"),
        ("Profundidade do Conteudo","---"),
    ];

    const COL_W_KDD: f32 = 89.0;
    const COL_GAP_KDD: f32 = 4.0;
    const COL2_X_KDD: f32 = MARGIN + COL_W_KDD + COL_GAP_KDD;
    const ITEM_H_KDD: f32 = 12.5;
    const ITEM_STEP: f32 = 14.0; // altura + gap entre itens
    const GRID_START_Y: f32 = 108.5; // topo do primeiro item

    for (i, (name, pct)) in kdd_items.iter().enumerate() {
        let col = i % 2;
        let row = i / 2;
        let ix = if col == 0 { MARGIN } else { COL2_X_KDD };
        let iy_top = GRID_START_Y - row as f32 * ITEM_STEP;
        let iy_bot = iy_top - ITEM_H_KDD;

        // Fundo do card
        cover.set_fill_color(Color::Rgb(Rgb { r: 0.94, g: 0.96, b: 1.00, icc_profile: None }));
        filled_rect(&cover, ix, iy_bot, ix + COL_W_KDD, iy_top);

        // Acento esquerdo azul
        cover.set_fill_color(Color::Rgb(Rgb { r: 0.22, g: 0.51, b: 0.94, icc_profile: None }));
        filled_rect(&cover, ix, iy_bot, ix + 2.5, iy_top);

        // Nome do critério
        dark_text(&cover);
        cover.use_text(*name, 8.0_f32, mm(ix + 5.0), mm(iy_bot + 4.0), &reg);

        // Badge de melhoria (direita)
        let badge_color = if pct.starts_with('+') {
            Color::Rgb(Rgb { r: 0.10, g: 0.40, b: 0.80, icc_profile: None })
        } else {
            Color::Rgb(Rgb { r: 0.55, g: 0.55, b: 0.60, icc_profile: None })
        };
        cover.set_fill_color(badge_color);
        cover.use_text(*pct, 8.5_f32, mm(ix + COL_W_KDD - 14.0), mm(iy_bot + 3.5), &bold);
    }

    // Rodapé da capa
    cover.set_fill_color(Color::Rgb(Rgb { r: 0.80, g: 0.80, b: 0.85, icc_profile: None }));
    filled_rect(&cover, MARGIN, 13.5, PAGE_W - MARGIN, 14.0);
    gray_text(&cover);
    cover.use_text("Gerado por GEO Analyzer", 7.5_f32, mm(MARGIN), mm(9.0), &reg);

    // ── Pages table ──────────────────────────────────────────────────────────
    // Colunas: URL/Titulo | Score | Palavras | Status
    const COL_URL_X: f32 = MARGIN;
    const COL_SCORE_X: f32 = 122.0;
    const COL_WORDS_X: f32 = 150.0;
    const COL_STATUS_X: f32 = 170.0;
    const ROW_H: f32 = 9.0;
    const HEADER_H: f32 = 10.0;
    const TABLE_TOP: f32 = 276.0;

    for (chunk_idx, chunk) in sorted.chunks(ROWS_PER_PAGE).enumerate() {
        let current_pdf_page = 2 + chunk_idx;
        let (tpg, tly) = doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "Table");
        let layer = doc.get_page(tpg).get_layer(tly);

        // Título da seção
        let section_title = if chunk_idx == 0 {
            "Paginas Analisadas".to_string()
        } else {
            format!("Paginas Analisadas  ({}/{})", chunk_idx + 1, table_pages_count)
        };
        dark_text(&layer);
        layer.use_text(&section_title, 14.0_f32, mm(MARGIN), mm(287.0), &bold);

        // Linha abaixo do título
        layer.set_fill_color(Color::Rgb(Rgb { r: 0.22, g: 0.51, b: 0.94, icc_profile: None }));
        filled_rect(&layer, MARGIN, 284.5, PAGE_W - MARGIN, 285.5);

        // Cabeçalho da tabela
        layer.set_fill_color(Color::Rgb(Rgb { r: 0.102, g: 0.196, b: 0.392, icc_profile: None }));
        filled_rect(&layer, MARGIN, TABLE_TOP - HEADER_H, PAGE_W - MARGIN, TABLE_TOP);

        white_text(&layer);
        layer.use_text("URL / Titulo",  8.0_f32, mm(COL_URL_X    + 2.0), mm(TABLE_TOP - HEADER_H + 3.0), &bold);
        layer.use_text("Score",         8.0_f32, mm(COL_SCORE_X  + 2.0), mm(TABLE_TOP - HEADER_H + 3.0), &bold);
        layer.use_text("Palavras",      8.0_f32, mm(COL_WORDS_X  + 2.0), mm(TABLE_TOP - HEADER_H + 3.0), &bold);
        layer.use_text("Status",        8.0_f32, mm(COL_STATUS_X + 2.0), mm(TABLE_TOP - HEADER_H + 3.0), &bold);

        // Linhas de dados
        for (row_idx, page) in chunk.iter().enumerate() {
            let row_y_top = TABLE_TOP - HEADER_H - (row_idx as f32) * ROW_H;
            let row_y_bot = row_y_top - ROW_H;
            let text_y = row_y_bot + 2.5;

            // zebra
            if row_idx % 2 == 0 {
                layer.set_fill_color(Color::Rgb(Rgb { r: 0.95, g: 0.96, b: 0.99, icc_profile: None }));
            } else {
                layer.set_fill_color(Color::Rgb(Rgb { r: 1.00, g: 1.00, b: 1.00, icc_profile: None }));
            }
            filled_rect(&layer, MARGIN, row_y_bot, PAGE_W - MARGIN, row_y_top);

            let score_pct = page.geo_score * 100.0;
            let title_str = page.title.as_deref().unwrap_or(&page.url);
            let display = trunc(title_str, 62);

            dark_text(&layer);
            layer.use_text(&display, 7.0_f32, mm(COL_URL_X + 2.0), mm(text_y), &reg);

            // Badge de score colorido
            layer.set_fill_color(score_color(score_pct));
            filled_rect(
                &layer,
                COL_SCORE_X, row_y_bot + 1.5,
                COL_SCORE_X + 16.0, row_y_top - 1.5,
            );
            white_text(&layer);
            layer.use_text(&format!("{:.0}", score_pct), 8.0_f32, mm(COL_SCORE_X + 3.0), mm(text_y), &bold);

            // Palavras
            layer.set_fill_color(Color::Rgb(Rgb { r: 0.25, g: 0.25, b: 0.25, icc_profile: None }));
            layer.use_text(&format!("{}", page.word_count), 7.5_f32, mm(COL_WORDS_X + 2.0), mm(text_y), &reg);

            // Status
            layer.set_fill_color(score_color(score_pct));
            layer.use_text(badge_label(score_pct), 7.5_f32, mm(COL_STATUS_X + 2.0), mm(text_y), &bold);
        }

        // Borda externa da tabela
        layer.set_outline_color(Color::Rgb(Rgb { r: 0.75, g: 0.75, b: 0.80, icc_profile: None }));
        let table_bottom = TABLE_TOP - HEADER_H - (chunk.len() as f32) * ROW_H;
        stroke_rect(&layer, MARGIN, table_bottom, PAGE_W - MARGIN, TABLE_TOP);

        draw_footer(&layer, &reg, current_pdf_page, total_pdf_pages);
    }

    // ── Per-page detail (optional) ───────────────────────────────────────────
    if include_detail {
        for (page_idx, page) in sorted.iter().enumerate() {
            let current_pdf_page = 1 + table_pages_count + 1 + page_idx;
            let (dpg, dly) = doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "Detail");
            let layer = doc.get_page(dpg).get_layer(dly);

            let title = page.title.as_deref().unwrap_or("(sem titulo)");
            let score_pct = page.geo_score * 100.0;

            // ── Faixa azul no topo ───────────────────────────────────────────
            layer.set_fill_color(Color::Rgb(Rgb { r: 0.102, g: 0.196, b: 0.392, icc_profile: None }));
            filled_rect(&layer, 0.0, 284.0, PAGE_W, PAGE_H);
            // acento colorido lateral
            layer.set_fill_color(Color::Rgb(Rgb { r: 0.22, g: 0.51, b: 0.94, icc_profile: None }));
            filled_rect(&layer, 0.0, 284.0, 5.0, PAGE_H);

            white_text(&layer);
            layer.use_text(&trunc(title, 70), 12.0_f32, mm(10.0), mm(290.5), &bold);
            layer.set_fill_color(Color::Rgb(Rgb { r: 0.75, g: 0.82, b: 0.96, icc_profile: None }));
            layer.use_text(&trunc(&page.url, 85), 7.5_f32, mm(10.0), mm(285.5), &reg);

            // ── Score destaque ───────────────────────────────────────────────
            // Cartão de score à direita
            let sc_card_x = PAGE_W - MARGIN - 42.0;
            layer.set_fill_color(score_color(score_pct));
            filled_rect(&layer, sc_card_x, 270.0, PAGE_W - MARGIN, 283.5);
            white_text(&layer);
            layer.use_text(
                &format!("Score: {:.0}/100", score_pct),
                11.0_f32, mm(sc_card_x + 3.0), mm(278.5), &bold,
            );
            layer.use_text(
                badge_label(score_pct),
                9.5_f32, mm(sc_card_x + 3.0), mm(272.5), &bold,
            );

            // Linha separadora
            layer.set_fill_color(Color::Rgb(Rgb { r: 0.75, g: 0.80, b: 0.90, icc_profile: None }));
            filled_rect(&layer, MARGIN, 267.5, PAGE_W - MARGIN, 268.5);

            // ── Critérios GEO ────────────────────────────────────────────────
            dark_text(&layer);
            layer.use_text("Criterios de Otimizacao GEO", 11.0_f32, mm(MARGIN), mm(263.0), &bold);

            // Cabeçalho das colunas
            const CRIT_LABEL_X: f32 = MARGIN;
            const CRIT_BAR_X: f32 = 100.0;
            const CRIT_BAR_W: f32 = 72.0;
            const CRIT_VAL_X: f32 = 175.0;
            const CRIT_HDR_H: f32 = 7.0;
            const CRIT_ROW_H: f32 = 13.5;
            const CRIT_HDR_BOT: f32 = 259.0 - CRIT_HDR_H; // = 252.0

            layer.set_fill_color(Color::Rgb(Rgb { r: 0.102, g: 0.196, b: 0.392, icc_profile: None }));
            filled_rect(&layer, MARGIN, CRIT_HDR_BOT, PAGE_W - MARGIN, 259.0);
            white_text(&layer);
            layer.use_text("Criterio",   8.5_f32, mm(CRIT_LABEL_X + 2.0), mm(CRIT_HDR_BOT + 2.0), &bold);
            layer.use_text("Percentual", 8.5_f32, mm(CRIT_BAR_X    + 2.0), mm(CRIT_HDR_BOT + 2.0), &bold);
            layer.use_text("Nota",       8.5_f32, mm(CRIT_VAL_X    + 2.0), mm(CRIT_HDR_BOT + 2.0), &bold);

            let criteria: &[(&str, &str, f64)] = &[
                ("Citacoes de Fontes",       "peso 20%", page.score_cite_sources),
                ("Citacoes Diretas",         "peso 20%", page.score_quotation_addition),
                ("Estatisticas",             "peso 15%", page.score_statistics_addition),
                ("Fluencia / Legibilidade",  "peso 15%", page.score_fluency),
                ("Tom Autoritativo",         "peso 10%", page.score_authoritative_tone),
                ("Termos Tecnicos",          "peso  8%", page.score_technical_terms),
                ("Clareza / Acessibilidade", "peso  7%", page.score_easy_to_understand),
                ("Estrutura de Conteudo",    "peso  3%", page.score_content_structure),
                ("Qualidade de Metadados",   "peso  2%", page.score_metadata_quality),
            ];

            let mut cy: f32 = CRIT_HDR_BOT; // começa do fundo do cabeçalho
            for (i, (name, weight, raw_score)) in criteria.iter().enumerate() {
                let sc_pct = raw_score * 100.0;
                let row_top = cy;
                let row_bot = cy - CRIT_ROW_H;
                // texto do nome: 2/3 de cima da linha; peso: terço inferior
                let name_y   = row_bot + CRIT_ROW_H * 0.62;
                let weight_y = row_bot + CRIT_ROW_H * 0.22;

                // zebra
                if i % 2 == 0 {
                    layer.set_fill_color(Color::Rgb(Rgb { r: 0.96, g: 0.97, b: 0.99, icc_profile: None }));
                } else {
                    layer.set_fill_color(Color::Rgb(Rgb { r: 1.00, g: 1.00, b: 1.00, icc_profile: None }));
                }
                filled_rect(&layer, MARGIN, row_bot, PAGE_W - MARGIN, row_top);

                // Nome
                dark_text(&layer);
                layer.use_text(*name, 8.5_f32, mm(CRIT_LABEL_X + 2.0), mm(name_y), &reg);
                // Peso
                gray_text(&layer);
                layer.use_text(*weight, 7.0_f32, mm(CRIT_LABEL_X + 2.0), mm(weight_y), &reg);

                // Barra de progresso centralizada verticalmente
                let bar_h = CRIT_ROW_H * 0.38;
                let bar_y  = row_bot + (CRIT_ROW_H - bar_h) / 2.0;
                progress_bar(&layer, CRIT_BAR_X, bar_y, CRIT_BAR_W, bar_h, sc_pct);

                // Valor numérico alinhado ao centro da linha
                let val_y = row_bot + CRIT_ROW_H * 0.42;
                layer.set_fill_color(score_color(sc_pct));
                layer.use_text(&format!("{:.0}", sc_pct), 10.0_f32, mm(CRIT_VAL_X + 4.0), mm(val_y), &bold);

                cy -= CRIT_ROW_H;
            }

            // Borda da tabela de critérios
            layer.set_outline_color(Color::Rgb(Rgb { r: 0.75, g: 0.75, b: 0.82, icc_profile: None }));
            stroke_rect(&layer, MARGIN, cy, PAGE_W - MARGIN, 259.0);

            // ── Resumo LLM ───────────────────────────────────────────────────
            cy -= 6.0;
            if let Some(summary) = &page.llm_summary {
                let wrapped = word_wrap(summary, 88);
                let line_count = wrapped.len().min(10);
                let line_h = 5.8_f32;
                let card_pad = 6.0_f32;
                let card_height = line_count as f32 * line_h + card_pad * 2.0 + 8.0;
                let card_bot = (cy - card_height).max(18.0);
                let card_top = card_bot + card_height;

                // Card background
                layer.set_fill_color(Color::Rgb(Rgb { r: 0.93, g: 0.96, b: 1.00, icc_profile: None }));
                filled_rect(&layer, MARGIN, card_bot, PAGE_W - MARGIN, card_top);

                // Borda esquerda colorida
                layer.set_fill_color(Color::Rgb(Rgb { r: 0.22, g: 0.51, b: 0.94, icc_profile: None }));
                filled_rect(&layer, MARGIN, card_bot, MARGIN + 3.0, card_top);

                // Título do card
                layer.set_fill_color(Color::Rgb(Rgb { r: 0.10, g: 0.30, b: 0.65, icc_profile: None }));
                layer.use_text(
                    "Resumo da Analise (Inteligencia Artificial):",
                    9.5_f32, mm(MARGIN + 5.0), mm(card_top - 6.5), &bold,
                );

                // Texto do resumo
                dark_text(&layer);
                let mut ty = card_top - 6.5 - card_pad;
                for line in wrapped.iter().take(10) {
                    if ty < card_bot + 2.0 { break; }
                    layer.use_text(line, 8.0_f32, mm(MARGIN + 5.0), mm(ty), &reg);
                    ty -= line_h;
                }
            }

            draw_footer(&layer, &reg, current_pdf_page, total_pdf_pages);
        }
    }

    let bytes = doc.save_to_bytes()?;
    Ok(bytes)
}
