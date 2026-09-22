//! Un servidor LSP de mentira para los tests de punta a punta del cliente.
//!
//! No es un ejemplo de uso: está en `examples/` porque así `cargo test` lo
//! compila solo, sin instalarse con `cargo install` ni entrar al `.deb`. Los
//! tests lo lanzan como lanzarían a rust-analyzer y manejan Flint como el
//! usuario.
//!
//!     lsp_falso <archivo espejo>
//!
//! Guarda su propia copia del documento aplicando lo que Flint le manda
//! (completo o por rangos en UTF-16), y después de cada cambio la escribe en
//! el archivo espejo: el test compara esa copia con la del editor, que es la
//! forma de saber que la sincronización no pierde ni corre nada.
//!
//! Las respuestas son fijas y fáciles de reconocer:
//! - diagnósticos: uno por cada `ERROR` del texto;
//! - autocompletado: `push`, reemplazando la palabra que hay en el cursor;
//! - hover: un bloque de código `fn falso()`;
//! - definición: línea 0, columna 3 del mismo archivo;
//! - renombre: todas las apariciones de la palabra del cursor;
//! - firmas: `fn suma(a: i32, b: i32)` adentro de un paréntesis sin cerrar
//!   de la línea, con el parámetro según las comas; afuera, nada;
//! - formato: saca los espacios del final de cada línea.

use std::io::{self, BufRead, BufReader, Write};

use serde_json::{Value, json};

fn leer(entrada: &mut impl BufRead) -> Option<Value> {
    let mut largo = None;
    loop {
        let mut linea = String::new();
        if entrada.read_line(&mut linea).ok()? == 0 {
            return None;
        }
        let linea = linea.trim_end();
        if linea.is_empty() {
            break;
        }
        if let Some(n) = linea.strip_prefix("Content-Length:") {
            largo = n.trim().parse().ok();
        }
    }
    let mut cuerpo = vec![0u8; largo?];
    entrada.read_exact(&mut cuerpo).ok()?;
    serde_json::from_slice(&cuerpo).ok()
}

fn escribir(salida: &mut impl Write, v: &Value) {
    let cuerpo = serde_json::to_vec(v).unwrap();
    let _ = write!(salida, "Content-Length: {}\r\n\r\n", cuerpo.len());
    let _ = salida.write_all(&cuerpo);
    let _ = salida.flush();
}

/// De (línea, columna UTF-16) a índice en bytes de `texto`.
fn byte_de(texto: &str, linea: usize, col16: usize) -> usize {
    let mut inicio = 0;
    for _ in 0..linea {
        match texto[inicio..].find('\n') {
            Some(p) => inicio += p + 1,
            None => return texto.len(),
        }
    }
    let mut unidades = 0;
    for (i, c) in texto[inicio..].char_indices() {
        if unidades >= col16 || c == '\n' {
            return inicio + i;
        }
        unidades += c.len_utf16();
    }
    texto.len()
}

/// La inversa: de índice en bytes a (línea, columna UTF-16).
fn posicion_de(texto: &str, byte: usize) -> Value {
    let antes = &texto[..byte];
    let linea = antes.matches('\n').count();
    let inicio = antes.rfind('\n').map_or(0, |p| p + 1);
    let col: usize = antes[inicio..].chars().map(char::len_utf16).sum();
    json!({"line": linea, "character": col})
}

fn rango(texto: &str, a: usize, b: usize) -> Value {
    json!({"start": posicion_de(texto, a), "end": posicion_de(texto, b)})
}

/// La palabra que toca `byte`: (inicio, fin) en bytes.
fn palabra_en(texto: &str, byte: usize) -> (usize, usize) {
    let es = |c: char| c.is_alphanumeric() || c == '_';
    let inicio = texto[..byte].char_indices().rev().take_while(|(_, c)| es(*c)).last().map_or(byte, |(i, _)| i);
    let fin = texto[byte..].char_indices().find(|(_, c)| !es(*c)).map_or(texto.len(), |(i, _)| byte + i);
    (inicio, fin)
}

fn diagnosticos(uri: &str, texto: &str) -> Value {
    let lista: Vec<Value> = texto
        .match_indices("ERROR")
        .map(|(i, _)| json!({"range": rango(texto, i, i + 5), "severity": 1, "message": "un ERROR"}))
        .collect();
    json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
           "params": {"uri": uri, "diagnostics": lista}})
}

fn main() {
    let espejo = std::env::args().nth(1).expect("falta el archivo espejo");
    let mut entrada = BufReader::new(io::stdin().lock());
    let mut salida = io::stdout().lock();
    let mut uri = String::new();
    let mut texto = String::new();

    while let Some(msg) = leer(&mut entrada) {
        let metodo = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let p = &msg["params"];
        let pos = || {
            let l = p["position"]["line"].as_u64().unwrap_or(0) as usize;
            let c = p["position"]["character"].as_u64().unwrap_or(0) as usize;
            (l, c)
        };
        let resultado = match metodo {
            "initialize" => json!({"capabilities": {
                "textDocumentSync": {"openClose": true, "change": 2},
                "completionProvider": {},
                "hoverProvider": true,
                "definitionProvider": true,
                "renameProvider": true,
                "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
                "documentFormattingProvider": true
            }}),
            "textDocument/didOpen" | "textDocument/didChange" => {
                if metodo == "textDocument/didOpen" {
                    uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                    texto = p["textDocument"]["text"].as_str().unwrap_or("").to_string();
                } else {
                    for cambio in p["contentChanges"].as_array().into_iter().flatten() {
                        let nuevo = cambio["text"].as_str().unwrap_or("");
                        match cambio.get("range") {
                            None => texto = nuevo.to_string(),
                            Some(r) => {
                                let byte = |q: &Value| {
                                    byte_de(&texto, q["line"].as_u64().unwrap() as usize, q["character"].as_u64().unwrap() as usize)
                                };
                                let (a, b) = (byte(&r["start"]), byte(&r["end"]));
                                texto.replace_range(a..b, nuevo);
                            }
                        }
                    }
                }
                std::fs::write(&espejo, &texto).unwrap();
                escribir(&mut salida, &diagnosticos(&uri, &texto));
                continue;
            }
            "textDocument/completion" => {
                let (l, c) = pos();
                let (a, b) = palabra_en(&texto, byte_de(&texto, l, c));
                json!([{"label": "push", "textEdit": {"range": rango(&texto, a, b), "newText": "push"}}])
            }
            "textDocument/hover" => json!({"contents": {"kind": "markdown", "value": "```rust\nfn falso()\n```"}}),
            "textDocument/definition" => {
                json!({"uri": uri, "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 4}}})
            }
            "textDocument/rename" => {
                let (l, c) = pos();
                let (a, b) = palabra_en(&texto, byte_de(&texto, l, c));
                let viejo = &texto[a..b];
                let nuevo = p["newName"].as_str().unwrap_or("");
                let cambios: Vec<Value> = texto
                    .match_indices(viejo)
                    .map(|(i, _)| json!({"range": rango(&texto, i, i + viejo.len()), "newText": nuevo}))
                    .collect();
                json!({"changes": {uri.clone(): cambios}})
            }
            "textDocument/signatureHelp" => {
                let (l, c) = pos();
                let byte = byte_de(&texto, l, c);
                let linea = &texto[texto[..byte].rfind('\n').map_or(0, |i| i + 1)..byte];
                let mut nivel = 0i32;
                let mut comas = 0;
                for ch in linea.chars() {
                    match ch {
                        '(' => {
                            nivel += 1;
                            comas = 0;
                        }
                        ')' => nivel -= 1,
                        ',' => comas += 1,
                        _ => {}
                    }
                }
                if nivel > 0 {
                    json!({"signatures": [{"label": "fn suma(a: i32, b: i32)",
                        "parameters": [{"label": [8, 14]}, {"label": [16, 22]}]}],
                        "activeSignature": 0, "activeParameter": comas})
                } else {
                    Value::Null
                }
            }
            "textDocument/formatting" => {
                let limpio: String = texto.lines().map(|l| format!("{}\n", l.trim_end())).collect();
                json!([{"range": rango(&texto, 0, texto.len()), "newText": limpio}])
            }
            "shutdown" => Value::Null,
            "exit" => return,
            _ => {
                // Notificaciones que no hacen falta (initialized, didSave…).
                if msg.get("id").is_none() {
                    continue;
                }
                Value::Null
            }
        };
        if let Some(id) = msg.get("id") {
            escribir(&mut salida, &json!({"jsonrpc": "2.0", "id": id, "result": resultado}));
        }
    }
}
