//! Reading JSON objects out of model replies (code fences, chatter around them).

use serde::de::DeserializeOwned;
use serde_json::Value;

/// First JSON object in `text`: the whole text, or the first balanced `{...}` that parses.
pub fn extract_object(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed)
        && v.is_object()
    {
        return Some(v);
    }
    let mut from = 0;
    while let Some(off) = trimmed[from..].find('{') {
        let start = from + off;
        if let Some(len) = balanced_len(&trimmed[start..])
            && let Ok(v) = serde_json::from_str::<Value>(&trimmed[start..start + len])
            && v.is_object()
        {
            return Some(v);
        }
        from = start + 1;
    }
    None
}

/// Byte length of the balanced object starting at `s[0] == '{'`, strings respected.
fn balanced_len(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Removes `null` members from every object, so `#[serde(default)]` applies to them.
pub fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|_, val| !val.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// Typed parse of the first JSON object; the error is a short pt-BR sentence that can be
/// sent back to the model.
pub fn parse<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let mut v = extract_object(text).ok_or_else(|| "nenhum objeto JSON na resposta".to_string())?;
    strip_nulls(&mut v);
    serde_json::from_value(v).map_err(|e| format!("JSON fora do formato pedido: {e}"))
}

/// A member as text: strings as they are, other values as JSON, missing/null as "".
pub fn str_field(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Like [`str_field`] but `None` when blank.
pub fn opt_str(v: &Value, key: &str) -> Option<String> {
    let s = str_field(v, key);
    if s.trim().is_empty() { None } else { Some(s.trim().to_string()) }
}

/// At most `max` characters, marking the cut.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str(" …[cortado]");
    out
}

/// First sentence (or line) of a text, for a short spoken line.
pub fn first_sentence(s: &str) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let line = line.trim_start_matches(['#', '-', '*', ' ']);
    match line.find(['.', '!', '?']) {
        Some(i) => line[..=i].to_string(),
        None => clip(line, 200),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_fenced_and_embedded_objects() {
        assert_eq!(extract_object(r#"{"a":1}"#).unwrap()["a"], 1);
        let fenced = "Claro!\n```json\n{\"kind\": \"chat\", \"reply\": \"oi {sim}\"}\n```\nfim";
        let v = extract_object(fenced).unwrap();
        assert_eq!(v["kind"], "chat");
        assert_eq!(v["reply"], "oi {sim}");
        let tricky = r#"texto { nao json } depois {"x": "chave } dentro", "y": {"z": 2}}"#;
        assert_eq!(extract_object(tricky).unwrap()["y"]["z"], 2);
        assert!(extract_object("sem json aqui").is_none());
        assert!(extract_object("[1,2,3]").is_none());
    }

    #[test]
    fn parse_strips_nulls_and_reports_errors() {
        #[derive(serde::Deserialize, Debug)]
        struct T {
            #[serde(default)]
            a: Vec<u8>,
            b: String,
        }
        let t: T = parse(r#"{"a": null, "b": "ok"}"#).unwrap();
        assert!(t.a.is_empty());
        assert_eq!(t.b, "ok");
        assert!(parse::<T>("nada").is_err());
        assert!(parse::<T>(r#"{"a": []}"#).unwrap_err().contains("formato"));
    }

    #[test]
    fn clip_and_first_sentence() {
        assert_eq!(clip("abc", 5), "abc");
        assert!(clip("ação longa", 3).starts_with("açã"));
        assert_eq!(first_sentence("## Título\nresto"), "Título");
        assert_eq!(first_sentence("Feito. Outra frase."), "Feito.");
    }
}
