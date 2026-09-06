//! Vista previa de Markdown: convierte el texto fuente en las mismas
//! `Line`/`Span` que Flint ya sabe dibujar, para poder verlo formateado sin
//! salir del editor.
//!
//! No es un editor WYSIWYG: es una vista de solo lectura del mismo buffer.
//! Editar sigue siendo sobre el texto fuente — que es lo que uno quiere en un
//! `.md`, donde la sintaxis *es* el contenido.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::editor::char_display_width;
use crate::highlight::{self, LanguageHighlighter};
use crate::theme::Theme;

/// Un trozo de texto con su estilo, todavía sin repartir en líneas.
struct Fragmento {
    texto: String,
    estilo: Style,
}

/// Acumula los fragmentos de un bloque y los reparte en líneas del ancho
/// disponible, cortando entre palabras.
struct Flujo {
    ancho: usize,
    /// Lo que se antepone a la primera línea del bloque (una viñeta, por
    /// ejemplo) y lo que se antepone a las siguientes (su sangría), para que
    /// un item de lista de tres renglones quede alineado bajo su viñeta.
    primera: Vec<Span<'static>>,
    siguientes: Vec<Span<'static>>,
    fragmentos: Vec<Fragmento>,
}

fn ancho_de(texto: &str) -> usize {
    texto.chars().map(|c| char_display_width(c, 0, 4)).sum()
}

fn ancho_spans(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|s| ancho_de(&s.content)).sum()
}

impl Flujo {
    fn nuevo(ancho: usize, primera: Vec<Span<'static>>, siguientes: Vec<Span<'static>>) -> Self {
        Flujo { ancho, primera, siguientes, fragmentos: Vec::new() }
    }

    fn empujar(&mut self, texto: &str, estilo: Style) {
        if texto.is_empty() {
            return;
        }
        self.fragmentos.push(Fragmento { texto: texto.to_string(), estilo });
    }

    /// Reparte lo acumulado en líneas. Corta entre palabras; una palabra más
    /// larga que el ancho disponible se deja desbordar en vez de partirla al
    /// medio (una URL cortada no se puede volver a pegar con la vista).
    fn terminar(self) -> Vec<Line<'static>> {
        let mut lineas: Vec<Line<'static>> = Vec::new();
        let mut actual: Vec<Span<'static>> = self.primera.clone();
        let mut usado = ancho_spans(&actual);
        let mut vacia = true;

        for frag in &self.fragmentos {
            // Se conservan los espacios como parte de las palabras para no
            // perder la separación al reensamblar.
            for palabra in frag.texto.split_inclusive(' ') {
                let w = ancho_de(palabra.trim_end());
                let disponible = self.ancho.saturating_sub(ancho_spans(&self.siguientes));
                if !vacia && usado + w > self.ancho.max(1) && disponible > 0 {
                    lineas.push(Line::from(std::mem::take(&mut actual)));
                    actual = self.siguientes.clone();
                    usado = ancho_spans(&actual);
                    // Una línea nueva no arranca con el espacio que separaba
                    // de la palabra anterior.
                    let limpia = palabra.trim_start();
                    if limpia.is_empty() {
                        continue;
                    }
                    usado += ancho_de(limpia);
                    actual.push(Span::styled(limpia.to_string(), frag.estilo));
                    vacia = false;
                    continue;
                }
                usado += ancho_de(palabra);
                actual.push(Span::styled(palabra.to_string(), frag.estilo));
                vacia = false;
            }
        }

        if !actual.is_empty() {
            lineas.push(Line::from(actual));
        }
        lineas
    }
}

/// Dibuja `source` como Markdown formateado, en líneas de `ancho` columnas.
///
/// `resaltador` es una fábrica: para el lenguaje de un cerco de código
/// (```rust), devuelve con qué resaltarlo. Así el código de adentro del
/// preview se ve igual que en el editor, reusando tree-sitter en vez de
/// mostrarlo plano.
pub fn render(
    source: &str,
    ancho: usize,
    theme: &Theme,
    resaltador: &dyn Fn(&str) -> Option<LanguageHighlighter>,
) -> Vec<Line<'static>> {
    let ancho = ancho.max(8);
    let mut salida: Vec<Line<'static>> = Vec::new();

    let mut opciones = Options::empty();
    opciones.insert(Options::ENABLE_STRIKETHROUGH);
    opciones.insert(Options::ENABLE_TABLES);
    opciones.insert(Options::ENABLE_TASKLISTS);

    // Estilo inline acumulado (negrita/cursiva/código se anidan).
    let mut estilo = Style::default().fg(theme.text_fg);
    let mut pila_estilos: Vec<Style> = Vec::new();

    let mut flujo: Option<Flujo> = None;
    // Nivel de anidamiento de listas y, por nivel, el próximo número si es
    // ordenada.
    let mut listas: Vec<Option<u64>> = Vec::new();
    let mut en_cita = false;
    let mut codigo: Option<(String, String)> = None; // (lenguaje, contenido)
    let mut destino = String::new();

    let dim = Style::default().fg(theme.dim);

    for evento in Parser::new_ext(source, opciones) {
        match evento {
            Event::Start(Tag::Heading { level, .. }) => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                if !salida.is_empty() {
                    salida.push(Line::from(""));
                }
                // La jerarquía se ve por sangría y por una marca, no por
                // tamaño: en una terminal todas las letras miden igual.
                let marca = Span::styled(
                    format!("{} ", "#".repeat(n)),
                    Style::default().fg(theme.dim),
                );
                estilo = Style::default()
                    .fg(theme.syn_keyword)
                    .add_modifier(Modifier::BOLD);
                flujo = Some(Flujo::nuevo(ancho, vec![marca], vec![Span::raw("  ".to_string())]));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(f) = flujo.take() {
                    salida.extend(f.terminar());
                }
                estilo = Style::default().fg(theme.text_fg);
            }

            Event::Start(Tag::Paragraph) => {
                if !salida.is_empty() {
                    salida.push(Line::from(""));
                }
                let sangria = sangria_de(&listas, en_cita, theme);
                flujo = Some(Flujo::nuevo(ancho, sangria.clone(), sangria));
            }
            Event::End(TagEnd::Paragraph) => {
                if let Some(f) = flujo.take() {
                    salida.extend(f.terminar());
                }
            }

            Event::Start(Tag::List(inicio)) => {
                listas.push(inicio);
                if listas.len() == 1 && !salida.is_empty() {
                    salida.push(Line::from(""));
                }
            }
            Event::End(TagEnd::List(_)) => {
                listas.pop();
            }

            Event::Start(Tag::Item) => {
                let nivel = listas.len().saturating_sub(1);
                let sangria_txt = "  ".repeat(nivel);
                let vinneta = match listas.last_mut() {
                    Some(Some(n)) => {
                        let txt = format!("{sangria_txt}{n}. ");
                        *n += 1;
                        txt
                    }
                    _ => format!("{sangria_txt}• "),
                };
                let ancho_vin = ancho_de(&vinneta);
                let primera = vec![Span::styled(vinneta, Style::default().fg(theme.syn_function))];
                let siguientes = vec![Span::raw(" ".repeat(ancho_vin))];
                flujo = Some(Flujo::nuevo(ancho, primera, siguientes));
            }
            Event::End(TagEnd::Item) => {
                if let Some(f) = flujo.take() {
                    salida.extend(f.terminar());
                }
            }

            Event::Start(Tag::BlockQuote(_)) => en_cita = true,
            Event::End(TagEnd::BlockQuote(_)) => en_cita = false,

            Event::Start(Tag::CodeBlock(kind)) => {
                let lenguaje = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                codigo = Some((lenguaje, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lenguaje, contenido)) = codigo.take() {
                    if !salida.is_empty() {
                        salida.push(Line::from(""));
                    }
                    salida.extend(bloque_de_codigo(&lenguaje, &contenido, theme, resaltador));
                }
            }

            Event::Start(Tag::Emphasis) => {
                pila_estilos.push(estilo);
                estilo = estilo.add_modifier(Modifier::ITALIC);
            }
            Event::Start(Tag::Strong) => {
                pila_estilos.push(estilo);
                estilo = estilo.add_modifier(Modifier::BOLD);
            }
            Event::Start(Tag::Strikethrough) => {
                pila_estilos.push(estilo);
                estilo = estilo.add_modifier(Modifier::CROSSED_OUT);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                pila_estilos.push(estilo);
                estilo = estilo
                    .fg(theme.syn_function)
                    .add_modifier(Modifier::UNDERLINED);
                destino = dest_url.to_string();
            }
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => {
                estilo = pila_estilos.pop().unwrap_or_default();
            }
            Event::End(TagEnd::Link) => {
                estilo = pila_estilos.pop().unwrap_or_default();
                // La URL va al lado, apagada: en una terminal no hay dónde
                // esconderla, y saber a dónde lleva un link es la mitad de
                // su utilidad.
                if let Some(f) = flujo.as_mut()
                    && !destino.is_empty()
                {
                    f.empujar(&format!(" ({destino})"), dim);
                }
                destino.clear();
            }

            Event::Text(txt) => {
                if let Some((_, contenido)) = codigo.as_mut() {
                    contenido.push_str(&txt);
                } else if let Some(f) = flujo.as_mut() {
                    f.empujar(&txt, estilo);
                }
            }
            Event::Code(txt) => {
                if let Some(f) = flujo.as_mut() {
                    f.empujar(&txt, Style::default().fg(theme.syn_string));
                }
            }
            Event::SoftBreak => {
                if let Some(f) = flujo.as_mut() {
                    f.empujar(" ", estilo);
                }
            }
            Event::HardBreak => {
                if let Some(f) = flujo.as_mut() {
                    f.empujar(" ", estilo);
                }
            }
            Event::Rule => {
                salida.push(Line::from(""));
                salida.push(Line::from(Span::styled("─".repeat(ancho.min(60)), dim)));
            }
            Event::TaskListMarker(hecho) => {
                if let Some(f) = flujo.as_mut() {
                    f.empujar(if hecho { "[x] " } else { "[ ] " }, dim);
                }
            }
            _ => {}
        }
    }

    if let Some(f) = flujo.take() {
        salida.extend(f.terminar());
    }
    if salida.is_empty() {
        salida.push(Line::from(Span::styled(
            "(documento vacío)",
            Style::default().fg(theme.dim),
        )));
    }
    salida
}

/// La sangría con la que arranca un párrafo según dónde esté: dentro de una
/// cita lleva su barra al margen, dentro de una lista se alinea con el texto
/// del item.
fn sangria_de(listas: &[Option<u64>], en_cita: bool, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if en_cita {
        spans.push(Span::styled("▏ ".to_string(), Style::default().fg(theme.dim)));
    }
    if !listas.is_empty() {
        spans.push(Span::raw("  ".repeat(listas.len())));
    }
    spans
}

/// Un cerco de código, con su contenido resaltado si Flint conoce el
/// lenguaje que declara.
fn bloque_de_codigo(
    lenguaje: &str,
    contenido: &str,
    theme: &Theme,
    resaltador: &dyn Fn(&str) -> Option<LanguageHighlighter>,
) -> Vec<Line<'static>> {
    let etiqueta = if lenguaje.is_empty() { "código".to_string() } else { lenguaje.to_string() };
    let mut lineas = vec![Line::from(Span::styled(
        format!("  ┌ {etiqueta}"),
        Style::default().fg(theme.dim),
    ))];

    let resaltados = resaltador(lenguaje)
        .map(|h| h.highlight_lines(contenido))
        .unwrap_or_default();

    for (i, texto) in contenido.lines().enumerate() {
        let mut spans = vec![Span::styled("  │ ".to_string(), Style::default().fg(theme.dim))];
        match resaltados.get(i) {
            Some(tramos) if !tramos.is_empty() => {
                let chars: Vec<char> = texto.chars().collect();
                let mut pos = 0usize;
                for &(ini, fin, kind) in tramos {
                    let ini = ini.min(chars.len());
                    let fin = fin.min(chars.len());
                    if ini > pos {
                        spans.push(Span::styled(
                            chars[pos..ini].iter().collect::<String>(),
                            Style::default().fg(theme.text_fg),
                        ));
                    }
                    if fin > ini {
                        spans.push(Span::styled(
                            chars[ini..fin].iter().collect::<String>(),
                            crate::ui::highlight_style(theme, kind),
                        ));
                        pos = fin;
                    }
                }
                if pos < chars.len() {
                    spans.push(Span::styled(
                        chars[pos..].iter().collect::<String>(),
                        Style::default().fg(theme.text_fg),
                    ));
                }
            }
            _ => spans.push(Span::styled(texto.to_string(), Style::default().fg(theme.text_fg))),
        }
        lineas.push(Line::from(spans));
    }

    lineas.push(Line::from(Span::styled(
        "  └".to_string(),
        Style::default().fg(theme.dim),
    )));
    lineas
}

/// Con qué resaltar el contenido de un cerco ```lenguaje, si Flint lo conoce.
pub fn resaltador_para(nombre: &str) -> Option<LanguageHighlighter> {
    highlight::lang_for_name(nombre).and_then(|l| LanguageHighlighter::new(&l))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El texto plano de cada línea renderizada, para poder afirmar sobre el
    /// resultado sin depender de los colores.
    fn texto(fuente: &str, ancho: usize) -> Vec<String> {
        let theme = Theme::default();
        render(fuente, ancho, &theme, &|_| None)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    }

    #[test]
    fn los_marcadores_desaparecen_y_queda_el_texto() {
        let v = texto("Un **fuerte** y *suave*.\n", 40);
        assert!(v.iter().any(|l| l == "Un fuerte y suave."), "{v:?}");
    }

    #[test]
    fn los_asteriscos_con_espacio_adentro_no_son_negrita() {
        // CommonMark: `** texto **` con espacios pegados a los asteriscos no
        // es énfasis, y los asteriscos quedan como texto literal. Sin
        // espacios sí lo es. Es la regla de "delimitadores flanqueantes".
        let con_espacios = texto("** Esta es una prueba **\n", 60);
        assert!(
            con_espacios.iter().any(|l| l.contains("**")),
            "los asteriscos se muestran tal cual: {con_espacios:?}"
        );
        let sin_espacios = texto("**Esta es una prueba**\n", 60);
        assert!(
            !sin_espacios.iter().any(|l| l.contains("**")),
            "acá sí es negrita y los marcadores desaparecen: {sin_espacios:?}"
        );
    }

    #[test]
    fn las_listas_llevan_vinneta() {
        let v = texto("- uno\n- dos\n", 40);
        assert!(v.iter().any(|l| l == "• uno"), "{v:?}");
        assert!(v.iter().any(|l| l == "• dos"), "{v:?}");
    }

    #[test]
    fn las_listas_ordenadas_se_numeran_solas() {
        let v = texto("1. uno\n1. dos\n1. tres\n", 40);
        assert!(v.iter().any(|l| l == "1. uno"), "{v:?}");
        assert!(v.iter().any(|l| l == "2. dos"), "{v:?}");
        assert!(v.iter().any(|l| l == "3. tres"), "{v:?}");
    }

    #[test]
    fn el_link_muestra_a_donde_va() {
        let v = texto("ver [el manual](https://ejemplo.cl)\n", 60);
        assert!(v.iter().any(|l| l.contains("el manual (https://ejemplo.cl)")), "{v:?}");
    }

    #[test]
    fn el_parrafo_se_reparte_segun_el_ancho() {
        let fuente = "palabra ".repeat(20);
        let angosto = texto(&fuente, 20);
        let ancho = texto(&fuente, 80);
        assert!(angosto.len() > ancho.len(), "más angosto = más líneas");
        // Y ninguna línea se pasa del ancho pedido.
        for l in &angosto {
            assert!(ancho_de(l) <= 20, "{l:?} se pasa de 20 columnas");
        }
    }

    #[test]
    fn el_cerco_de_codigo_queda_en_su_recuadro() {
        let v = texto("```rust\nfn main() {}\n```\n", 40);
        assert!(v.iter().any(|l| l.contains("┌ rust")), "{v:?}");
        assert!(v.iter().any(|l| l.contains("│ fn main() {}")), "{v:?}");
    }

    #[test]
    fn un_documento_vacio_lo_dice() {
        let v = texto("", 40);
        assert_eq!(v, vec!["(documento vacío)".to_string()]);
    }
}
