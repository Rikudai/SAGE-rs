//! Parser incremental de Server-Sent Events: devolve o conteúdo de cada linha `data:`.

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Alimenta bytes brutos (podem cortar caracteres UTF-8 ao meio) e devolve os `data:` completos.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            if let Some(d) = data_line(&line) {
                out.push(d);
            }
        }
        out
    }

    /// Fim do fluxo: processa uma última linha sem `\n`.
    pub fn finish(&mut self) -> Vec<String> {
        let rest = std::mem::take(&mut self.buf);
        data_line(&rest).into_iter().collect()
    }
}

fn data_line(raw: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(raw);
    let s = s.trim_end_matches(['\r', '\n']);
    let rest = s.strip_prefix("data:")?;
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    if rest.is_empty() { None } else { Some(rest.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: {\"a\":").is_empty());
        let out = p.feed(b"1}\r\n\r\ndata: [DONE]\n");
        assert_eq!(out, vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
    }

    #[test]
    fn keeps_split_utf8_and_ignores_other_fields() {
        let mut p = SseParser::new();
        let full = "data: olá\n".as_bytes();
        let (a, b) = full.split_at(8); // corta no meio do "á"
        assert!(p.feed(a).is_empty());
        let mut out = p.feed(b);
        out.extend(p.finish());
        assert_eq!(out, vec!["olá".to_string()]);
        assert!(p.feed(b": comentario\nevent: x\n").is_empty());
    }

    #[test]
    fn finish_flushes_last_line() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: fim").is_empty());
        assert_eq!(p.finish(), vec!["fim".to_string()]);
    }
}
