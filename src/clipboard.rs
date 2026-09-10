//! El portapapeles, con dos caminos y una razón para cada uno.
//!
//! `arboard` habla con el portapapeles del sistema, que es lo correcto
//! cuando hay un servidor gráfico. En una sesión SSH sin display no hay tal
//! cosa: `arboard` no abre, y hasta ahora copiar no llegaba a ningún lado.
//!
//! El otro camino es OSC 52, una secuencia de escape que le pide a la
//! **terminal** que guarde el texto. Como la terminal es la del que está
//! sentado adelante, copiar dentro de un `flint` corriendo en un servidor
//! remoto termina en el portapapeles de la máquina local, que es justo lo
//! que uno quiere y lo que `arboard` no puede dar.
//!
//! Leer no tiene equivalente: OSC 52 define una consulta, pero casi ninguna
//! terminal la habilita (deja que cualquier programa remoto lea lo que
//! copiaste, así que la apagan por seguridad) y la respuesta llegaría
//! mezclada con las teclas. Sin portapapeles del sistema, pegar usa el
//! registro interno de Flint.

use std::io::{self, Write};

/// De dónde sale y a dónde va lo que se copia.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// El del sistema si se pudo abrir; si no, la terminal por OSC 52.
    #[default]
    Auto,
    /// Solo el del sistema (`arboard`).
    System,
    /// Solo la terminal (OSC 52), aunque haya servidor gráfico — es lo que
    /// se quiere cuando Flint corre dentro de un contenedor o un `tmux`
    /// remoto y el "sistema" no es la máquina de adelante.
    Terminal,
    /// Ninguno de los dos: solo el registro interno.
    Internal,
}

impl Mode {
    pub fn from_name(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Mode::Auto),
            "system" | "sistema" => Some(Mode::System),
            "terminal" | "osc52" => Some(Mode::Terminal),
            "internal" | "interno" | "none" => Some(Mode::Internal),
            _ => None,
        }
    }
}

/// Tope de texto que se manda por OSC 52. La secuencia viaja por el mismo
/// canal que lo que se dibuja, y las terminales cortan las que se pasan de
/// largo (el límite varía y ninguna avisa). Con un tope propio, un texto
/// enorme se reporta como no copiado en vez de llegar a medias del otro lado.
const OSC52_MAX_BYTES: usize = 64 * 1024;

/// Le pide a la terminal que guarde `text` en el portapapeles del sistema
/// donde está corriendo — la máquina de adelante, no la remota.
pub fn copy_via_terminal(text: &str) -> Result<(), String> {
    if text.len() > OSC52_MAX_BYTES {
        return Err(format!(
            "son {} KiB y la terminal no acepta más de {} KiB por OSC 52",
            text.len() / 1024,
            OSC52_MAX_BYTES / 1024
        ));
    }
    // `52;c` es el portapapeles ("clipboard") propiamente dicho; el texto va
    // en base64 porque la secuencia termina con un carácter de control y
    // cualquier byte crudo podría cortarla antes de tiempo.
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = io::stdout();
    out.write_all(seq.as_bytes()).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

/// Base64 estándar, con relleno. Son veinte líneas y evita una dependencia
/// más para el único lugar del programa que lo necesita.
fn base64(datos: &[u8]) -> String {
    const TABLA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(datos.len().div_ceil(3) * 4);
    for trozo in datos.chunks(3) {
        let b = [
            trozo[0],
            *trozo.get(1).unwrap_or(&0),
            *trozo.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        let indices = [n >> 18 & 63, n >> 12 & 63, n >> 6 & 63, n & 63];
        for (i, &idx) in indices.iter().enumerate() {
            // Con uno o dos bytes de entrada sobran caracteres de salida: los
            // que no representan ningún bit real se escriben como relleno.
            if i <= trozo.len() {
                out.push(TABLA[idx as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_coincide_con_los_casos_del_rfc() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_no_se_marea_con_acentos_ni_emoji() {
        assert_eq!(base64("ñ".as_bytes()), "w7E=");
        assert_eq!(base64("😀".as_bytes()), "8J+YgA==");
    }

    #[test]
    fn un_texto_enorme_se_rechaza_en_vez_de_llegar_cortado() {
        let grande = "x".repeat(OSC52_MAX_BYTES + 1);
        assert!(copy_via_terminal(&grande).is_err());
    }

    #[test]
    fn los_nombres_del_modo_son_los_del_archivo_de_configuracion() {
        assert_eq!(Mode::from_name("auto"), Some(Mode::Auto));
        assert_eq!(Mode::from_name("Terminal"), Some(Mode::Terminal));
        assert_eq!(Mode::from_name("osc52"), Some(Mode::Terminal));
        assert_eq!(Mode::from_name("cualquiera"), None);
    }
}
