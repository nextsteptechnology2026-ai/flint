use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use serde_json::{json, Value};

/// Qué esperamos hacer con la respuesta a una petición que nosotros mandamos.
pub enum Pending {
    Initialize,
    /// Índice del buffer que pidió el autocompletado — con varios documentos
    /// abiertos en el mismo servidor, la respuesta tiene que volver al buffer
    /// que la pidió, no necesariamente al que esté activo cuando llegue.
    /// `trigger` es la posición (línea, columna) donde empieza el
    /// identificador ya tipeado antes de pedirla (no el cursor) — de ahí se
    /// borra y se inserta lo elegido al aceptar una sugerencia; `prefix` es
    /// ese identificador, para que el popup arranque ya filtrado.
    Completion {
        buffer: usize,
        trigger: (usize, usize),
        prefix: String,
    },
}

/// Un cambio de rango para `did_change_incremental` — posiciones en
/// (línea, columna UTF-16, como pide LSP), no genéricas para no acoplar
/// este módulo al `Position` de `editor`.
pub struct LspContentChange {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub text: String,
}

pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next_id: u64,
    pub pending: std::collections::HashMap<u64, Pending>,
    doc_version: i32,
}

fn encode_uri_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn file_uri(path: &Path) -> String {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let display = abs.display().to_string();
    let display = if display.starts_with('/') {
        display
    } else {
        format!("/{display}")
    };
    format!("file://{}", encode_uri_path(&display))
}

fn write_message(stdin: &mut ChildStdin, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
    stdin.write_all(&body)?;
    stdin.flush()
}

fn read_loop(stdout: impl Read, tx: mpsc::Sender<Value>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) => {}
                Err(_) => return,
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(rest) = line.strip_prefix("Content-Length:") {
                content_length = rest.trim().parse().ok();
            }
        }
        let Some(len) = content_length else { continue };
        let mut buf = vec![0u8; len];
        if reader.read_exact(&mut buf).is_err() {
            return;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&buf)
            && tx.send(v).is_err()
        {
            return;
        }
    }
}

impl LspClient {
    /// Lanza el proceso del servidor. `Err` normalmente significa "el
    /// comando no existe en el PATH" (`ErrorKind::NotFound`).
    pub fn spawn(cmd: &str) -> io::Result<LspClient> {
        let mut child = Command::new(cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || read_loop(stdout, tx));

        Ok(LspClient {
            child,
            stdin,
            rx,
            next_id: 0,
            pending: std::collections::HashMap::new(),
            doc_version: 1,
        })
    }

    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn request(&mut self, method: &str, params: Value, pending: Pending) -> io::Result<u64> {
        let id = self.alloc_id();
        self.pending.insert(id, pending);
        write_message(
            &mut self.stdin,
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )?;
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> io::Result<()> {
        write_message(
            &mut self.stdin,
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
    }

    pub fn initialize(&mut self, root_uri: &str) -> io::Result<u64> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "synchronization": { "didSave": true },
                    "publishDiagnostics": { "relatedInformation": false },
                    "completion": { "completionItem": { "snippetSupport": false } }
                }
            }
        });
        self.request("initialize", params, Pending::Initialize)
    }

    pub fn send_initialized(&mut self) -> io::Result<()> {
        self.notify("initialized", json!({}))
    }

    pub fn did_open(&mut self, uri: &str, language_id: &str, text: &str) -> io::Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": self.doc_version,
                    "text": text,
                }
            }),
        )
    }

    /// Sincronización completa: manda el documento entero. Es el único modo
    /// que todo servidor LSP soporta sin excepción, así que sigue siendo el
    /// fallback para deshacer/rehacer, reemplazar todo, ediciones
    /// multi-cursor, y para cualquier servidor que no anuncie soporte
    /// incremental en `initialize`.
    pub fn did_change(&mut self, uri: &str, text: &str) -> io::Result<()> {
        self.doc_version += 1;
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": self.doc_version },
                "contentChanges": [ { "text": text } ]
            }),
        )
    }

    /// Sincronización incremental: manda solo los rangos que cambiaron, en
    /// vez del documento entero — mucho menos tráfico y trabajo del lado del
    /// servidor en archivos grandes con ediciones chicas (el caso normal:
    /// tipear, borrar). Los rangos vienen en el mismo orden en que pasaron
    /// de verdad, que es justo lo que el protocolo espera (cada entrada de
    /// `contentChanges` se interpreta sobre el resultado de aplicar la
    /// anterior, no todas contra el documento original).
    pub fn did_change_incremental(&mut self, uri: &str, edits: &[LspContentChange]) -> io::Result<()> {
        self.doc_version += 1;
        let changes: Vec<Value> = edits
            .iter()
            .map(|e| {
                json!({
                    "range": {
                        "start": { "line": e.start.0, "character": e.start.1 },
                        "end": { "line": e.end.0, "character": e.end.1 },
                    },
                    "text": e.text,
                })
            })
            .collect();
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": self.doc_version },
                "contentChanges": changes
            }),
        )
    }

    pub fn did_close(&mut self, uri: &str) -> io::Result<()> {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        )
    }

    pub fn request_completion(
        &mut self,
        uri: &str,
        line: usize,
        character: usize,
        buffer: usize,
        trigger: (usize, usize),
        prefix: String,
    ) -> io::Result<u64> {
        self.request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character }
            }),
            Pending::Completion { buffer, trigger, prefix },
        )
    }

    /// Respuesta genérica a una petición que el propio servidor nos manda
    /// (p. ej. `client/registerCapability`, `workspace/configuration`) para
    /// que nunca se quede esperando algo que Flint no necesita interpretar.
    pub fn respond_default(&mut self, id: Value, method: &str, params: &Value) {
        let result = if method == "workspace/configuration" {
            let n = params
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Value::Array(vec![Value::Null; n])
        } else {
            Value::Null
        };
        let _ = write_message(&mut self.stdin, &json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    pub fn try_recv(&self) -> Option<Value> {
        self.rx.try_recv().ok()
    }

    /// Espera bloqueante (con límite) solo para el arranque, cuando todavía
    /// no hay nada más que hacer que confirmar que el servidor respondió.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<Value> {
        self.rx.recv_timeout(timeout).ok()
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
