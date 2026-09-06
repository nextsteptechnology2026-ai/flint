use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::editor::{Editor, HighlightKind, Layer, LineEnding, Mode, Position, Selection, Severity};
use crate::theme::Theme;

/// El estilo completo de un tramo resaltado. Casi todas las categorías son
/// solo un color, pero las de texto con formato (Markdown) se distinguen por
/// forma: un título en negrita, un link subrayado. Poner *color* donde va
/// negrita sería traducir mal — la negrita ya existe en la terminal.
///
/// No agregan claves nuevas al `theme.toml`: reusan colores que el tema ya
/// define, así un tema viejo sigue funcionando tal cual.
fn highlight_style(theme: &Theme, kind: HighlightKind) -> Style {
    let base = Style::default();
    match kind {
        HighlightKind::Heading => base
            .fg(theme.syn_keyword)
            .add_modifier(Modifier::BOLD),
        HighlightKind::Strong => base.fg(theme.text_fg).add_modifier(Modifier::BOLD),
        HighlightKind::Emphasis => base.fg(theme.text_fg).add_modifier(Modifier::ITALIC),
        HighlightKind::Link => base
            .fg(theme.syn_function)
            .add_modifier(Modifier::UNDERLINED),
        other => base.fg(highlight_color(theme, other)),
    }
}

fn highlight_color(theme: &Theme, kind: HighlightKind) -> ratatui::style::Color {
    match kind {
        HighlightKind::Keyword => theme.syn_keyword,
        HighlightKind::Function => theme.syn_function,
        HighlightKind::Type => theme.syn_type,
        HighlightKind::String => theme.syn_string,
        HighlightKind::Comment => theme.syn_comment,
        HighlightKind::Number => theme.syn_number,
        HighlightKind::Constant => theme.syn_constant,
        HighlightKind::Operator => theme.syn_operator,
        HighlightKind::Punctuation => theme.syn_punctuation,
        HighlightKind::Variable => theme.text_fg,
        HighlightKind::Property => theme.syn_property,
        HighlightKind::Attribute => theme.syn_attribute,
        // Las de Markdown nunca llegan acá: las resuelve `highlight_style`
        // entero, porque no son solo color.
        HighlightKind::Heading => theme.syn_keyword,
        HighlightKind::Strong | HighlightKind::Emphasis => theme.text_fg,
        HighlightKind::Link => theme.syn_function,
    }
}

fn severity_color(theme: &Theme, sev: Severity) -> ratatui::style::Color {
    match sev {
        Severity::Error => theme.error,
        Severity::Warning => theme.warning,
        Severity::Info | Severity::Hint => theme.hint,
    }
}

fn severity_mark(sev: Severity) -> &'static str {
    match sev {
        Severity::Error => "✖",
        Severity::Warning => "▲",
        Severity::Info | Severity::Hint => "●",
    }
}

/// Lo que `draw` necesita devolverle a `main` para mapear clics del mouse:
/// el área de texto (siempre) y la de la barra de pestañas (solo si se
/// dibujó — con un único buffer no hay pestañas que clickear).
pub struct DrawAreas {
    pub text_area: Rect,
    pub tabs_area: Option<Rect>,
}

/// Dibuja el frame completo. `tab_labels`/`active_tab` solo importan cuando
/// hay más de un buffer abierto — con uno solo la barra de pestañas no ocupa
/// línea, igual que antes de tener multi-buffer. `completion_matches` es la
/// lista YA filtrada por lo tipeado desde que se abrió el popup (mismo
/// esquema que `palette_matches`): acá solo se dibuja, el filtrado vive en
/// `main`, donde también vive el de la paleta.
pub fn draw(
    f: &mut Frame,
    ed: &mut Editor,
    palette_matches: &[String],
    completion_matches: &[(String, Option<String>)],
    tab_labels: &[String],
    active_tab: usize,
    theme: &Theme,
) -> DrawAreas {
    let size = f.area();
    let show_tabs = tab_labels.len() > 1;
    let mut constraints = vec![Constraint::Length(1)];
    if show_tabs {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(1));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(2));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(size);

    let title_area = chunks[0];
    let mut i = 1;
    let mut tabs_area = None;
    if show_tabs {
        draw_tabs(f, tab_labels, active_tab, chunks[i], theme);
        tabs_area = Some(chunks[i]);
        i += 1;
    }
    let text_area = chunks[i];
    let message_area = chunks[i + 1];
    let help_area = chunks[i + 2];

    draw_title(f, ed, title_area, theme);
    draw_text(f, ed, text_area, theme);
    draw_message(f, ed, message_area, theme);
    draw_help(f, ed, help_area, theme);

    if let Mode::Completion { selected, .. } = &ed.mode {
        draw_completion_popup(f, text_area, completion_matches, *selected, theme);
    }
    if let Mode::Palette { selected, .. } = &ed.mode {
        draw_palette_popup(f, text_area, palette_matches, *selected, theme);
    }

    DrawAreas { text_area, tabs_area }
}

/// Barra de pestañas: solo se dibuja cuando hay más de un buffer abierto. La
/// pestaña activa se resalta con el color de selección; el resto usa el
/// color apagado de las barras — el mismo lenguaje visual que ya usaba el
/// resto de la interfaz para "esto no es el foco actual".
fn draw_tabs(f: &mut Frame, labels: &[String], active: usize, area: Rect, theme: &Theme) {
    let mut spans = Vec::with_capacity(labels.len() * 2);
    for (i, label) in labels.iter().enumerate() {
        let style = if i == active {
            theme.bar_style().add_modifier(Modifier::BOLD)
        } else {
            theme.bar_style().fg(theme.dim)
        };
        spans.push(Span::styled(format!(" {} ", label), style));
        spans.push(Span::styled("│", Style::default().fg(theme.dim)));
    }
    let p = Paragraph::new(Line::from(spans)).style(theme.bar_style());
    f.render_widget(p, area);
}

/// Ancho en columnas de cada pestaña tal como las dibuja `draw_tabs`
/// (`" {label} "` + el separador `"│"`) — un solo cálculo compartido entre
/// dibujar y detectar clics, para que nunca se desincronicen entre sí.
fn tab_widths(labels: &[String]) -> Vec<u16> {
    labels.iter().map(|l| l.chars().count() as u16 + 3).collect()
}

/// A qué buffer corresponde un clic en la columna `col` de la barra de
/// pestañas (`area` es la que devolvió `draw` en `DrawAreas::tabs_area`),
/// si cae dentro de alguna.
pub fn tab_at_column(labels: &[String], area: Rect, col: u16) -> Option<usize> {
    if col < area.x {
        return None;
    }
    let mut x = area.x;
    for (i, w) in tab_widths(labels).into_iter().enumerate() {
        if col < x + w {
            return Some(i);
        }
        x += w;
    }
    None
}

fn draw_title(f: &mut Frame, ed: &Editor, area: Rect, theme: &Theme) {
    let name = ed
        .filename
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "[Sin nombre]".to_string());
    let mark = if ed.dirty { "  •" } else { "" };
    let diag_summary = summarize_diagnostics(ed);
    let layer_tag = match ed.layer {
        Layer::Direct => "",
        Layer::ModalNormal => "  [NORMAL]",
        Layer::ModalInsert => "  [INSERT]",
    };
    let cursor_count = 1 + ed.secondary.len();
    let cursor_tag = if cursor_count > 1 {
        format!("  ×{cursor_count} cursores")
    } else {
        String::new()
    };
    // Solo se marca CRLF — LF es la convención por defecto y más común, así
    // que dejarla muda (en vez de mostrar "LF" siempre) es menos ruido en la
    // barra para el caso común, igual que hacen la mayoría de los editores.
    let eol_tag = if ed.line_ending == LineEnding::CrLf { "  CRLF" } else { "" };
    let wrap_tag = if ed.wrap { "  [wrap]" } else { "" };
    let text = format!(" Flint   {name}{mark}{diag_summary}{layer_tag}{cursor_tag}{eol_tag}{wrap_tag}");
    let p = Paragraph::new(text).style(theme.bar_style().add_modifier(Modifier::BOLD));
    f.render_widget(p, area);
}

fn summarize_diagnostics(ed: &Editor) -> String {
    if ed.diagnostics.is_empty() {
        return String::new();
    }
    let errors = ed
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = ed
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    format!("   ✖{errors} ▲{warnings}")
}

fn draw_message(f: &mut Frame, ed: &Editor, area: Rect, theme: &Theme) {
    let text = match &ed.mode {
        Mode::Prompt { label, buffer, .. } => format!(" {label}{buffer}"),
        Mode::Completion { items, .. } => {
            format!(
                " {} sugerencia(s) — ↑↓ elegir, Enter/Tab insertar, Esc cancelar",
                items.len()
            )
        }
        Mode::Palette { query, .. } => format!(" Paleta de comandos › {query}"),
        Mode::Editing => format!(" {}", ed.status),
    };
    let p = Paragraph::new(text).style(Style::default().fg(theme.bar_fg));
    f.render_widget(p, area);
}

fn draw_help(f: &mut Frame, ed: &Editor, area: Rect, theme: &Theme) {
    let lines = match ed.layer {
        Layer::Direct => vec![
            Line::from(" ^S Guardar  ^Q Salir  ^F Buscar  ^R Reemplazar  ^Z Deshacer  ^Y Rehacer  ^C/^X/^V Copiar/Cortar/Pegar"),
            Line::from(
                " F2 Capa modal  ^P Paleta  ^Espacio Autocompletar  ^G Diagnóstico  ^D +cursor  Alt+clic +cursor  Shift/Mouse Seleccionar",
            ),
        ],
        Layer::ModalNormal => vec![
            Line::from(
                " w Palabra  x Línea  n Nodo(sintaxis)  d Cortar  c Cambiar  y Copiar  p Pegar  u/U Deshacer/Rehacer",
            ),
            Line::from(
                " i/a/I/A Insertar  o/O Abrir línea  ^D +cursor  Alt+clic +cursor  Esc Deseleccionar  F2 Modo directo",
            ),
        ],
        Layer::ModalInsert => vec![
            Line::from(" INSERT — escribe normalmente. Esc vuelve a NORMAL.  F2 Modo directo"),
            Line::from(" ^S Guardar  ^Q Salir  ^F Buscar  ^R Reemplazar  ^Z Deshacer  ^Y Rehacer"),
        ],
    };
    let style = theme.bar_style().fg(theme.dim);
    let p = Paragraph::new(lines).style(style);
    f.render_widget(p, area);
}

fn draw_completion_popup(
    f: &mut Frame,
    text_area: Rect,
    items: &[(String, Option<String>)],
    selected: usize,
    theme: &Theme,
) {
    let visible = items.len().clamp(1, 8);
    let width = items
        .iter()
        .take(visible)
        .map(|(label, detail)| label.chars().count() + detail.as_ref().map_or(0, |d| d.chars().count() + 3))
        .max()
        .unwrap_or(10)
        .clamp(12, text_area.width.saturating_sub(4) as usize) as u16
        // +2 los bordes del recuadro, +1 el espacio con el que arranca cada
        // línea (`" {label}"`) — sin contarlo, la etiqueta más larga siempre
        // perdía su último carácter.
        + 3;
    let height = visible as u16 + 2;
    let x = text_area.x + 2;
    let y = (text_area.y + text_area.height.saturating_sub(height)).min(text_area.y + text_area.height.saturating_sub(1));
    let area = Rect {
        x: x.min(text_area.x + text_area.width.saturating_sub(width)),
        y,
        width: width.min(text_area.width),
        height: height.min(text_area.height),
    };

    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.popup_bg).fg(theme.dim));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = if items.is_empty() {
        vec![Line::from(Span::styled(
            " sin coincidencias",
            Style::default().fg(theme.dim),
        ))]
    } else {
        items
            .iter()
            .take(visible)
            .enumerate()
            .map(|(i, (label, detail))| {
                let style = if i == selected {
                    Style::default().bg(theme.selection_bg).fg(theme.selection_fg)
                } else {
                    Style::default().fg(theme.text_fg)
                };
                let text = match detail {
                    Some(d) => format!(" {label}  {d}"),
                    None => format!(" {label}"),
                };
                Line::from(Span::styled(text, style))
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), inner);
}

/// La paleta de comandos se ancla arriba del área de texto (como en la
/// mayoría de los editores), no cerca del cursor — no tiene una posición
/// natural en el buffer, a diferencia del autocompletado.
fn draw_palette_popup(f: &mut Frame, text_area: Rect, matches: &[String], selected: usize, theme: &Theme) {
    let visible = matches.len().clamp(1, 10);
    let width = matches
        .iter()
        .take(visible)
        .map(|m| m.chars().count())
        .max()
        .unwrap_or(20)
        .clamp(24, text_area.width.saturating_sub(4) as usize) as u16
        // Ver el comentario del popup de autocompletado: bordes + el espacio
        // inicial de cada línea.
        + 3;
    let height = visible as u16 + 2;
    let x = text_area.x + (text_area.width.saturating_sub(width)) / 2;
    let area = Rect {
        x,
        y: text_area.y + 1,
        width: width.min(text_area.width),
        height: height.min(text_area.height),
    };

    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" comandos ")
        .style(Style::default().bg(theme.popup_bg).fg(theme.dim));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = if matches.is_empty() {
        vec![Line::from(Span::styled(
            " sin coincidencias",
            Style::default().fg(theme.dim),
        ))]
    } else {
        matches
            .iter()
            .take(visible)
            .enumerate()
            .map(|(i, label)| {
                let style = if i == selected {
                    Style::default().bg(theme.selection_bg).fg(theme.selection_fg)
                } else {
                    Style::default().fg(theme.text_fg)
                };
                Line::from(Span::styled(format!(" {label}"), style))
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_text(f: &mut Frame, ed: &mut Editor, area: Rect, theme: &Theme) {
    if ed.wrap {
        draw_text_wrapped(f, ed, area, theme);
    } else {
        draw_text_scroll(f, ed, area, theme);
    }
}

/// Sin ajuste de línea: una línea lógica es siempre una fila de pantalla: las
/// que no entran se cortan, con scroll horizontal (`col_offset`) para
/// llegar a lo que quedó afuera. El comportamiento de siempre, sin cambios.
fn draw_text_scroll(f: &mut Frame, ed: &mut Editor, area: Rect, theme: &Theme) {
    let visible_rows = area.height as usize;
    let gutter_w = ed.gutter_width();
    let diag_col_w: u16 = 2;
    let content_w = area
        .width
        .saturating_sub(gutter_w)
        .saturating_sub(diag_col_w) as usize;

    if visible_rows > 0 {
        if ed.cursor.line < ed.row_offset {
            ed.row_offset = ed.cursor.line;
        }
        if ed.cursor.line >= ed.row_offset + visible_rows {
            ed.row_offset = ed.cursor.line + 1 - visible_rows;
        }
    }
    // `col_offset` y todo lo que sigue están en columnas de pantalla, no en
    // índices de carácter: un `\t` es un solo carácter pero ocupa varias
    // columnas, así que el cursor y el scroll horizontal tienen que hablar en
    // las columnas que de verdad se dibujan.
    let cursor_vis = ed.display_col(ed.cursor.line, ed.cursor.col);
    if content_w > 0 {
        if cursor_vis < ed.col_offset {
            ed.col_offset = cursor_vis;
        }
        if cursor_vis >= ed.col_offset + content_w {
            ed.col_offset = cursor_vis + 1 - content_w;
        }
    } else {
        ed.col_offset = cursor_vis;
    }

    let all_sels = ed.selections_snapshot();
    let line_count = ed.line_count();
    let mut lines: Vec<Line> = Vec::with_capacity(visible_rows.max(1));

    for i in 0..visible_rows {
        let line_idx = ed.row_offset + i;
        if line_idx >= line_count {
            let gutter = Span::styled(" ".repeat((gutter_w + diag_col_w) as usize), Style::default());
            lines.push(Line::from(vec![
                gutter,
                Span::styled("~", Style::default().fg(theme.dim)),
            ]));
            continue;
        }
        let raw = ed.rope.line(line_idx).to_string();
        let content: &str = raw.trim_end_matches(['\n', '\r']);
        let chars: Vec<char> = content.chars().collect();

        let diag_span = match ed.diagnostic_severity_for_line(line_idx) {
            Some(sev) => Span::styled(
                format!("{} ", severity_mark(sev)),
                Style::default().fg(severity_color(theme, sev)),
            ),
            None => Span::raw("  "),
        };

        let gutter_text = format!(
            "{:>width$} ",
            line_idx + 1,
            width = (gutter_w as usize).saturating_sub(1)
        );
        let gutter_span = Span::styled(gutter_text, Style::default().fg(theme.dim));

        // Todas las selecciones que caen en esta línea: rangos con el estilo
        // de selección, y cursores secundarios sin rango con un carácter
        // resaltado en su propio color — el cursor real de la terminal solo
        // puede estar en un lugar (el de la primaria).
        let mut sel_overlays: Vec<(usize, usize, Style)> = Vec::new();
        for (idx, sel) in all_sels.iter().enumerate() {
            let Some((s, e)) = selection_cols_on_line(sel, line_idx, chars.len()) else {
                continue;
            };
            if e > s {
                let style = Style::default().bg(theme.selection_bg).fg(theme.selection_fg);
                sel_overlays.push((s, e, style));
            } else if idx != 0 && s < chars.len() {
                let style = Style::default().bg(theme.secondary_bg).fg(theme.secondary_fg);
                sel_overlays.push((s, s + 1, style));
            }
        }

        let highlights = ed
            .highlights_by_line
            .get(line_idx)
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        let diag_overlays: Vec<(usize, usize, ratatui::style::Color)> = ed
            .diagnostics
            .iter()
            .filter_map(|d| {
                diagnostic_cols_on_line(d, line_idx, chars.len())
                    .map(|(s, e)| (s, e, severity_color(theme, d.severity)))
            })
            .collect();

        let line = LineCells::new(&chars, ed.tab_width);
        let mut spans = vec![diag_span, gutter_span];
        spans.extend(build_line_spans(
            &line,
            ed.col_offset,
            content_w,
            highlights,
            &sel_overlays,
            &diag_overlays,
            theme,
        ));
        lines.push(Line::from(spans));
    }

    let p = Paragraph::new(lines).style(Style::default().fg(theme.text_fg));
    f.render_widget(p, area);

    let cursor_col_visible = cursor_vis.saturating_sub(ed.col_offset);
    let cursor_x = area.x + diag_col_w + gutter_w + cursor_col_visible as u16;
    let cursor_y = area.y + (ed.cursor.line.saturating_sub(ed.row_offset)) as u16;
    if cursor_x < area.x + area.width && cursor_y < area.y + area.height {
        f.set_cursor_position((cursor_x, cursor_y));
    }
}

/// Con ajuste de línea: cada línea lógica larga se parte en varias filas de
/// pantalla en vez de desplazarse horizontalmente — no hay `col_offset`
/// (siempre se ve todo el ancho de cada línea, tarde o temprano). Solo la
/// primera fila de cada línea lógica lleva número/marca de diagnóstico en el
/// margen; las siguientes son continuación.
fn draw_text_wrapped(f: &mut Frame, ed: &mut Editor, area: Rect, theme: &Theme) {
    let visible_rows = area.height as usize;
    let gutter_w = ed.gutter_width();
    let diag_col_w: u16 = 2;
    let content_w = area
        .width
        .saturating_sub(gutter_w)
        .saturating_sub(diag_col_w) as usize;
    let line_count = ed.line_count();

    if ed.cursor.line < ed.row_offset {
        ed.row_offset = ed.cursor.line;
    }
    // A diferencia del modo sin ajuste, acá una línea lógica puede ocupar
    // varias filas — hay que contar filas visuales entre `row_offset` y el
    // cursor, no líneas, para saber si el cursor sigue entrando en pantalla.
    // Si una sola línea (la del cursor) ya es más alta que la pantalla
    // entera, se muestra desde su principio — verla completa haría falta
    // desplazamiento dentro de una misma línea, que queda afuera de esta
    // primera versión de ajuste.
    let cursor_vis = ed.display_col(ed.cursor.line, ed.cursor.col);
    if visible_rows > 0 && content_w > 0 {
        loop {
            let mut rows_used = 0usize;
            let mut cursor_fits = false;
            let last = ed.cursor.line.min(line_count.saturating_sub(1));
            for line in ed.row_offset..=last {
                let starts = ed.wrap_row_starts(line, content_w);
                let line_rows = starts.len();
                if line == ed.cursor.line {
                    let cursor_sub = ed.wrap_row_of(&starts, cursor_vis);
                    rows_used += cursor_sub + 1;
                    cursor_fits = rows_used <= visible_rows;
                } else {
                    rows_used += line_rows;
                }
                if rows_used > visible_rows {
                    break;
                }
            }
            if cursor_fits || ed.row_offset >= ed.cursor.line {
                break;
            }
            ed.row_offset += 1;
        }
    }
    ed.col_offset = 0;

    let all_sels = ed.selections_snapshot();
    let mut lines: Vec<Line> = Vec::with_capacity(visible_rows.max(1));
    let mut cursor_screen: Option<(u16, u16)> = None;
    let mut row = 0usize;
    let mut line_idx = ed.row_offset;

    while row < visible_rows {
        if line_idx >= line_count {
            let gutter = Span::styled(" ".repeat((gutter_w + diag_col_w) as usize), Style::default());
            lines.push(Line::from(vec![
                gutter,
                Span::styled("~", Style::default().fg(theme.dim)),
            ]));
            row += 1;
            line_idx += 1;
            continue;
        }

        let raw = ed.rope.line(line_idx).to_string();
        let content: &str = raw.trim_end_matches(['\n', '\r']);
        let chars: Vec<char> = content.chars().collect();

        let mut sel_overlays: Vec<(usize, usize, Style)> = Vec::new();
        for (idx, sel) in all_sels.iter().enumerate() {
            let Some((s, e)) = selection_cols_on_line(sel, line_idx, chars.len()) else {
                continue;
            };
            if e > s {
                let style = Style::default().bg(theme.selection_bg).fg(theme.selection_fg);
                sel_overlays.push((s, e, style));
            } else if idx != 0 && s < chars.len() {
                let style = Style::default().bg(theme.secondary_bg).fg(theme.secondary_fg);
                sel_overlays.push((s, s + 1, style));
            }
        }
        let highlights = ed
            .highlights_by_line
            .get(line_idx)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let diag_overlays: Vec<(usize, usize, ratatui::style::Color)> = ed
            .diagnostics
            .iter()
            .filter_map(|d| {
                diagnostic_cols_on_line(d, line_idx, chars.len())
                    .map(|(s, e)| (s, e, severity_color(theme, d.severity)))
            })
            .collect();

        let line = LineCells::new(&chars, ed.tab_width);
        // Los cortes los decide el editor (`wrap_row_starts`), no una grilla
        // fija cada `content_w`: una fila termina antes si el carácter
        // siguiente no entra entero.
        let starts = ed.wrap_row_starts(line_idx, content_w.max(1));
        for (sub, &col_from) in starts.iter().enumerate() {
            if row >= visible_rows {
                break;
            }
            let col_to = starts.get(sub + 1).copied().unwrap_or_else(|| line.len());
            let show_gutter = sub == 0;
            let diag_span = if show_gutter {
                match ed.diagnostic_severity_for_line(line_idx) {
                    Some(sev) => Span::styled(
                        format!("{} ", severity_mark(sev)),
                        Style::default().fg(severity_color(theme, sev)),
                    ),
                    None => Span::raw("  "),
                }
            } else {
                Span::raw("  ")
            };
            let gutter_span = if show_gutter {
                let gutter_text = format!(
                    "{:>width$} ",
                    line_idx + 1,
                    width = (gutter_w as usize).saturating_sub(1)
                );
                Span::styled(gutter_text, Style::default().fg(theme.dim))
            } else {
                Span::styled(" ".repeat(gutter_w as usize), Style::default())
            };

            let mut spans = vec![diag_span, gutter_span];
            spans.extend(build_line_spans(
                &line,
                col_from,
                col_to.saturating_sub(col_from),
                highlights,
                &sel_overlays,
                &diag_overlays,
                theme,
            ));
            lines.push(Line::from(spans));

            if line_idx == ed.cursor.line {
                let cursor_sub = ed.wrap_row_of(&starts, cursor_vis);
                if cursor_sub == sub {
                    let cursor_col_in_row = cursor_vis - col_from;
                    cursor_screen = Some((
                        area.x + diag_col_w + gutter_w + cursor_col_in_row as u16,
                        area.y + row as u16,
                    ));
                }
            }
            row += 1;
        }
        line_idx += 1;
    }

    let p = Paragraph::new(lines).style(Style::default().fg(theme.text_fg));
    f.render_widget(p, area);

    if let Some((x, y)) = cursor_screen
        && x < area.x + area.width
        && y < area.y + area.height
    {
        f.set_cursor_position((x, y));
    }
}

/// Columnas locales [inicio,fin) que ocupa una selección en esta línea, si la
/// toca. `end==start` significa "sin rango, solo el cursor en esa columna".
fn selection_cols_on_line(sel: &Selection, line_idx: usize, line_len: usize) -> Option<(usize, usize)> {
    let (start, end) = if (sel.anchor.line, sel.anchor.col) <= (sel.cursor.line, sel.cursor.col) {
        (sel.anchor, sel.cursor)
    } else {
        (sel.cursor, sel.anchor)
    };
    if line_idx < start.line || line_idx > end.line {
        return None;
    }
    let s = if line_idx == start.line { start.col } else { 0 };
    let e = if line_idx == end.line { end.col } else { line_len };
    Some((s, e))
}

/// Columnas locales [inicio,fin) que ocupa un diagnóstico en esta línea —
/// mismo patrón que `selection_cols_on_line`, pero con el rango que reportó
/// el servidor (`start_col`..`end_col`, o toda la línea en las intermedias
/// si el diagnóstico cruza más de una). Un rango vacío del servidor
/// (columna igual a igual) se ensancha a un carácter para que se vea algo.
fn diagnostic_cols_on_line(
    d: &crate::editor::Diagnostic,
    line_idx: usize,
    line_len: usize,
) -> Option<(usize, usize)> {
    if line_idx < d.line || line_idx > d.end_line {
        return None;
    }
    let s = if line_idx == d.line { d.start_col.min(line_len) } else { 0 };
    let mut e = if line_idx == d.end_line { d.end_col.min(line_len) } else { line_len };
    if e <= s {
        e = (s + 1).min(line_len).max(s);
    }
    Some((s, e))
}

/// Qué se dibuja en una columna de la pantalla.
enum CellKind {
    /// Los caracteres `chars[inicio..fin]`: normalmente uno solo, y más de
    /// uno cuando le siguen marcas combinantes (un acento que se pinta sobre
    /// la letra sin ocupar columna propia) — van pegadas a su letra para que
    /// no se pierdan por el camino.
    Text(usize, usize),
    /// Un espacio propio: cada una de las columnas en que se expande un `\t`.
    Blank,
    /// La segunda mitad de un carácter ancho (CJK, emoji). La columna está
    /// ocupada por el glifo de la celda anterior, así que acá no se dibuja
    /// nada: la celda existe solo para que las cuentas de columnas cierren.
    WideTail,
}

/// Una columna de pantalla y el índice del carácter del que salió (`src`),
/// que es lo que permite casar los rangos de resaltado, selección y
/// diagnósticos — que vienen en caracteres — con lo que de verdad se dibuja.
struct Cell {
    kind: CellKind,
    src: usize,
}

/// Una línea ya expandida a las columnas que de verdad se dibujan. Es el
/// puente entre las dos unidades que conviven en el editor: el buffer cuenta
/// caracteres (un `\t` es uno, un `日` es uno) y la terminal cuenta columnas
/// (ese `\t` puede ocupar cuatro, ese `日` ocupa dos). Con esto, el dibujo y
/// los rangos de resaltado se pueden mezclar sin que nadie tenga que
/// acordarse de la diferencia.
struct LineCells<'a> {
    cells: Vec<Cell>,
    chars: &'a [char],
    tab_width: usize,
}

impl<'a> LineCells<'a> {
    fn new(chars: &'a [char], tab_width: usize) -> Self {
        let mut cells: Vec<Cell> = Vec::with_capacity(chars.len());
        for (i, &c) in chars.iter().enumerate() {
            let width = crate::editor::char_display_width(c, cells.len(), tab_width);
            if width == 0 {
                // Marca combinante: se estira el texto de la celda anterior
                // para que la incluya. Si la línea arranca con una (no
                // debería, pero pasa con archivos rotos), se le da una
                // columna propia en vez de tirarla — mejor que se vea rara a
                // que desaparezca.
                match cells.last_mut() {
                    Some(Cell { kind: CellKind::Text(_, end), .. }) => *end = i + 1,
                    _ => cells.push(Cell { kind: CellKind::Text(i, i + 1), src: i }),
                }
                continue;
            }
            let head = if c == '\t' { CellKind::Blank } else { CellKind::Text(i, i + 1) };
            cells.push(Cell { kind: head, src: i });
            for _ in 1..width {
                let kind = if c == '\t' { CellKind::Blank } else { CellKind::WideTail };
                cells.push(Cell { kind, src: i });
            }
        }
        LineCells { cells, chars, tab_width }
    }

    /// Cuántas columnas de pantalla ocupa la línea entera.
    fn len(&self) -> usize {
        self.cells.len()
    }

    /// El texto de un tramo de columnas, listo para meter en un `Span`.
    ///
    /// Un carácter ancho cortado por el borde de la ventana (scroll
    /// horizontal o ajuste de línea) se dibuja como un espacio: medio glifo
    /// no se puede dibujar, y ocupar su columna es lo que mantiene alineado
    /// todo lo que sigue. Lo mismo del otro lado, cuando el tramo arranca en
    /// la segunda mitad de un carácter que quedó afuera.
    fn text(&self, from: usize, to: usize) -> String {
        let run = &self.cells[from..to];
        let mut text = String::with_capacity(run.len());
        for (k, cell) in run.iter().enumerate() {
            match cell.kind {
                CellKind::Blank => text.push(' '),
                CellKind::Text(s, e) => {
                    let width = crate::editor::char_display_width(self.chars[s], 0, self.tab_width);
                    if k + width <= run.len() {
                        text.extend(&self.chars[s..e]);
                    } else {
                        text.push(' ');
                    }
                }
                // Si no es la primera celda del tramo, su glifo ya se dibujó.
                CellKind::WideTail => {
                    if k == 0 {
                        text.push(' ');
                    }
                }
            }
        }
        text
    }

    /// Traduce un rango de caracteres `[s,e)` (como vienen el resaltado, las
    /// selecciones y los diagnósticos) al rango de columnas que le
    /// corresponde. Los `src` no decrecen nunca, así que alcanza con una
    /// búsqueda binaria en cada extremo.
    fn cell_range(&self, s: usize, e: usize) -> (usize, usize) {
        (
            self.cells.partition_point(|c| c.src < s),
            self.cells.partition_point(|c| c.src < e),
        )
    }
}

fn build_line_spans(
    line: &LineCells,
    col_offset: usize,
    content_w: usize,
    highlights: &[(usize, usize, HighlightKind)],
    selections: &[(usize, usize, Style)],
    diagnostics: &[(usize, usize, ratatui::style::Color)],
    theme: &Theme,
) -> Vec<Span<'static>> {
    let total = line.len();
    let start = col_offset.min(total);
    let end = (col_offset + content_w).min(total);
    if start >= end {
        return vec![Span::raw(String::new())];
    }
    let width = end - start;
    let mut styles: Vec<Style> = vec![Style::default().fg(theme.text_fg); width];

    for &(hs, he, kind) in highlights {
        let (hs, he) = line.cell_range(hs, he);
        let s = hs.max(start);
        let e = he.min(end);
        if s < e {
            let style = highlight_style(theme, kind);
            for slot in &mut styles[s - start..e - start] {
                *slot = style;
            }
        }
    }

    for &(ss, se, style) in selections {
        let (ss, se) = line.cell_range(ss, se);
        let s = ss.max(start);
        let e = se.min(end);
        if s < e {
            for slot in &mut styles[s - start..e - start] {
                *slot = style;
            }
        }
    }

    // El subrayado de diagnóstico se suma sin pisar el color de fondo/letra
    // que ya tenía la celda (sintaxis o selección) — no reemplaza el estilo,
    // solo le agrega el modificador y el color de subrayado.
    for &(ds, de, color) in diagnostics {
        let (ds, de) = line.cell_range(ds, de);
        let s = ds.max(start);
        let e = de.min(end);
        if s < e {
            for slot in &mut styles[s - start..e - start] {
                *slot = slot.underline_color(color).add_modifier(Modifier::UNDERLINED);
            }
        }
    }

    let mut spans = Vec::new();
    let mut i = 0;
    while i < width {
        let style = styles[i];
        let mut j = i + 1;
        while j < width && styles[j] == style {
            j += 1;
        }
        let text = line.text(start + i, start + j);
        spans.push(Span::styled(text, style));
        i = j;
    }
    spans
}

/// Convierte una coordenada de pantalla (columna, fila) del mouse a una posición en el buffer.
pub fn screen_to_pos(ed: &Editor, text_area: Rect, col: u16, row: u16) -> Option<Position> {
    if text_area.width == 0 || text_area.height == 0 {
        return None;
    }
    if row < text_area.y || row >= text_area.y + text_area.height {
        return None;
    }
    if col < text_area.x || col >= text_area.x + text_area.width {
        return None;
    }
    let diag_col_w: u16 = 2;
    let gutter = ed.gutter_width();
    let content_x = text_area.x + diag_col_w + gutter;
    let rel_row = (row - text_area.y) as usize;

    if !ed.wrap {
        let line = (ed.row_offset + rel_row).min(ed.line_count().saturating_sub(1));
        let vis_col = if col >= content_x {
            ed.col_offset + (col - content_x) as usize
        } else {
            0
        };
        return Some(Position { line, col: ed.col_from_display(line, vis_col) });
    }

    // Con ajuste, varias filas de pantalla pueden ser la misma línea lógica
    // — hay que recorrer desde `row_offset` acumulando filas visuales por
    // línea hasta encontrar en cuál cae `rel_row`, igual que al dibujar.
    let content_w = text_area
        .width
        .saturating_sub(gutter)
        .saturating_sub(diag_col_w) as usize;
    let line_count = ed.line_count();
    let mut remaining = rel_row;
    let mut line = ed.row_offset;
    loop {
        if line >= line_count {
            let last = line_count.saturating_sub(1);
            return Some(Position { line: last, col: ed.line_char_len(last) });
        }
        // Los mismos cortes que usó el dibujo: si acá se recalcularan de otra
        // forma, un clic caería en un carácter distinto del que se ve.
        let starts = ed.wrap_row_starts(line, content_w.max(1));
        if remaining < starts.len() {
            let col_in_sub = if col >= content_x { (col - content_x) as usize } else { 0 };
            let vis_col = starts[remaining] + col_in_sub;
            return Some(Position { line, col: ed.col_from_display(line, vis_col) });
        }
        remaining -= starts.len();
        line += 1;
    }
}

/// Tests de la expansión de una línea a columnas de pantalla — el punto donde
/// se juntan tabuladores, caracteres anchos y marcas combinantes, y donde un
/// error se ve como texto corrido en vez de como un error.
#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn un_caracter_ancho_ocupa_dos_columnas() {
        let c = chars("日x");
        let line = LineCells::new(&c, 4);
        assert_eq!(line.len(), 3);
        assert_eq!(line.text(0, 3), "日x");
    }

    #[test]
    fn un_caracter_ancho_partido_por_el_borde_se_dibuja_como_espacio() {
        let c = chars("日x");
        let line = LineCells::new(&c, 4);
        // Solo entra su primera mitad: medio glifo no se puede dibujar.
        assert_eq!(line.text(0, 1), " ");
        // Y del otro lado, arrancando en la segunda mitad del que quedó afuera.
        assert_eq!(line.text(1, 3), " x");
    }

    #[test]
    fn las_marcas_combinantes_viajan_con_su_letra() {
        let c = chars("e\u{301}x");
        let line = LineCells::new(&c, 4);
        assert_eq!(line.len(), 2, "el acento no ocupa columna propia");
        assert_eq!(line.text(0, 2), "e\u{301}x", "y no se pierde por el camino");
    }

    #[test]
    fn el_tabulador_se_dibuja_como_espacios_hasta_la_parada() {
        let c = chars("a\tb");
        let line = LineCells::new(&c, 4);
        assert_eq!(line.len(), 5);
        assert_eq!(line.text(0, 5), "a   b");
    }

    #[test]
    fn los_rangos_en_caracteres_se_traducen_a_columnas() {
        let c = chars("a\t日b");
        let line = LineCells::new(&c, 4);
        // El tabulador es el carácter 1 y ocupa las columnas 1..4.
        assert_eq!(line.cell_range(1, 2), (1, 4));
        // El CJK es el carácter 2 y ocupa las columnas 4..6: un resaltado que
        // lo toque tiene que pintar las dos, no media letra.
        assert_eq!(line.cell_range(2, 3), (4, 6));
    }
}
