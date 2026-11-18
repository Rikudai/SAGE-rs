//! Leitura tolerante de argumentos JSON (modelos às vezes mandam números como texto).

use serde_json::Value;

/// Texto obrigatório e não vazio (após trim).
pub fn req_str(a: &Value, key: &str) -> Result<String, String> {
    match a.get(key).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => Err(format!("Argumento obrigatório ausente ou vazio: '{key}'.")),
    }
}

/// Texto obrigatório que pode ser vazio e não sofre trim (conteúdo de arquivo, etc.).
pub fn req_str_raw(a: &Value, key: &str) -> Result<String, String> {
    a.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("Argumento obrigatório ausente: '{key}'."))
}

pub fn opt_str(a: &Value, key: &str) -> Option<String> {
    a.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

pub fn opt_i64(a: &Value, key: &str) -> Option<i64> {
    match a.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.round() as i64)),
        Value::String(s) => s.trim().parse::<f64>().ok().map(|f| f.round() as i64),
        _ => None,
    }
}

pub fn opt_bool(a: &Value, key: &str) -> bool {
    match a.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.to_lowercase().as_str(), "true" | "sim" | "1"),
        _ => false,
    }
}

pub fn req_i32(a: &Value, key: &str) -> Result<i32, String> {
    let v = opt_i64(a, key).ok_or_else(|| format!("Argumento numérico obrigatório ausente: '{key}'."))?;
    i32::try_from(v).map_err(|_| format!("Valor fora do intervalo em '{key}'."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_are_tolerant() {
        let a = json!({"x": "10", "y": 5.4, "z": 7});
        assert_eq!(req_i32(&a, "x"), Ok(10));
        assert_eq!(req_i32(&a, "y"), Ok(5));
        assert_eq!(req_i32(&a, "z"), Ok(7));
        assert!(req_i32(&a, "w").is_err());
    }

    #[test]
    fn strings() {
        let a = json!({"s": "  oi ", "e": " "});
        assert_eq!(req_str(&a, "s").as_deref(), Ok("oi"));
        assert!(req_str(&a, "e").is_err());
        assert!(opt_str(&a, "e").is_none());
        assert!(req_str_raw(&a, "e").is_ok());
    }
}
