//! Python: a call with the `requests` library. Export only.

use super::{PartBody, Parts};
use serde_json::Value;

/// A double-quoted Python string. JSON's escaping is valid Python.
fn quote(s: &str) -> String {
    Value::from(s).to_string()
}

/// A JSON value as a Python literal (`true` is `True`, `null` is `None`).
fn literal(value: &Value, indent: usize) -> String {
    let pad = " ".repeat(indent + 4);
    let close = " ".repeat(indent);
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => quote(s),
        Value::Array(items) if items.is_empty() => "[]".into(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(|v| format!("{pad}{}", literal(v, indent + 4))).collect();
            format!("[\n{}\n{close}]", inner.join(",\n"))
        }
        Value::Object(fields) if fields.is_empty() => "{}".into(),
        Value::Object(fields) => {
            let inner: Vec<String> = fields.iter().map(|(k, v)| format!("{pad}{}: {}", quote(k), literal(v, indent + 4))).collect();
            format!("{{\n{}\n{close}}}", inner.join(",\n"))
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(path)
}

pub fn export(p: &Parts) -> String {
    let method = p.method.to_ascii_lowercase();
    let known = ["get", "post", "put", "patch", "delete", "head", "options"].contains(&method.as_str());
    let mut args: Vec<String> = Vec::new();

    // a header sent twice: the values are joined, as HTTP allows for most headers
    let mut headers: Vec<(String, Vec<String>)> = Vec::new();
    for (name, value) in &p.headers {
        match headers.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some((_, values)) => values.push(value.clone()),
            None => headers.push((name.clone(), vec![value.clone()])),
        }
    }
    let is_json = headers.iter().any(|(n, v)| n.eq_ignore_ascii_case("content-type") && v.iter().any(|x| x.to_ascii_lowercase().contains("json")));
    if !headers.is_empty() {
        let rows: Vec<String> = headers.iter().map(|(n, v)| format!("        {}: {},", quote(n), quote(&v.join(", ")))).collect();
        args.push(format!("    headers={{\n{}\n    }}", rows.join("\n")));
    }

    match &p.body {
        PartBody::None => {}
        PartBody::Text(text) if is_json => match serde_json::from_str::<Value>(text) {
            Ok(value) => args.push(format!("    json={}", literal(&value, 4))),
            // not JSON as written (a {{variable}} outside quotes): send the text
            Err(_) => args.push(format!("    data={}", quote(text))),
        },
        PartBody::Text(text) | PartBody::Form(text) => args.push(format!("    data={}", quote(text))),
        PartBody::Multipart(fields) => {
            let rows: Vec<String> = fields
                .iter()
                .map(|(name, is_file, value)| {
                    if *is_file {
                        format!("        {}: ({}, open({}, \"rb\")),", quote(name), quote(file_name(value)), quote(value))
                    } else {
                        format!("        {}: (None, {}),", quote(name), quote(value))
                    }
                })
                .collect();
            args.push(format!("    files={{\n{}\n    }}", rows.join("\n")));
        }
    }
    if !p.follow_redirects {
        args.push("    allow_redirects=False".into());
    }
    if p.insecure_tls {
        args.push("    verify=False".into());
    }

    let call = if known { format!("requests.{method}({}", quote(&p.url)) } else { format!("requests.request({}, {}", quote(&p.method), quote(&p.url)) };
    let mut out = String::from("import requests\n\n");
    if args.is_empty() {
        out.push_str(&format!("response = {call})\n"));
    } else {
        out.push_str(&format!("response = {call},\n{},\n)\n", args.join(",\n")));
    }
    out.push_str("print(response.status_code)\nprint(response.text)");
    out
}

#[cfg(test)]
mod tests {
    use super::super::{export, Dialect};
    use crate::domain::model::{BodyMode, FieldKind, FormField, PersistedState};

    fn py(state: &PersistedState) -> String {
        export(state, Dialect::PythonRequests)
    }

    #[test]
    fn a_get_is_a_bare_call() {
        let state = PersistedState { method: "get".into(), url: "https://h/items?page=2".into(), ..Default::default() };
        assert_eq!(
            py(&state),
            "import requests\n\nresponse = requests.get(\"https://h/items?page=2\")\nprint(response.status_code)\nprint(response.text)"
        );
    }

    #[test]
    fn a_json_body_is_a_python_literal() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/orders".into(),
            headers_text: "Authorization: Bearer {{token}}".into(),
            body_mode: BodyMode::Json,
            json_body: r#"{"name":"widget","ok":true,"gone":null,"tags":["a"],"n":1.5}"#.into(),
            ..Default::default()
        };
        let text = py(&state);
        assert!(text.contains("requests.post(\"https://h/orders\""), "{text}");
        assert!(text.contains("\"Authorization\": \"Bearer {{token}}\""), "{text}");
        assert!(text.contains("json={"), "{text}");
        assert!(text.contains("\"ok\": True") && text.contains("\"gone\": None") && text.contains("\"n\": 1.5"), "{text}");
        assert!(!text.contains("true") && !text.contains("null"), "{text}");
    }

    #[test]
    fn a_body_that_is_not_json_as_written_stays_text() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/x".into(),
            body_mode: BodyMode::Json,
            json_body: "{\"id\": {{id}}}".into(),
            ..Default::default()
        };
        assert!(py(&state).contains(r#"data="{\"id\": {{id}}}""#));
    }

    #[test]
    fn options_forms_files_and_odd_methods() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/upload".into(),
            body_mode: BodyMode::Multipart,
            multipart_fields: vec![
                FormField { key: "title".into(), kind: FieldKind::Text, value: "my doc".into(), enabled: true },
                FormField { key: "file".into(), kind: FieldKind::File, value: "C:/data/a.txt".into(), enabled: true },
            ],
            follow_redirects: false,
            insecure_tls: true,
            ..Default::default()
        };
        let text = py(&state);
        assert!(text.contains("\"title\": (None, \"my doc\")"), "{text}");
        assert!(text.contains("\"file\": (\"a.txt\", open(\"C:/data/a.txt\", \"rb\"))"), "{text}");
        assert!(text.contains("allow_redirects=False") && text.contains("verify=False"), "{text}");
        let odd = PersistedState { method: "PURGE".into(), url: "https://h/cache".into(), ..Default::default() };
        assert!(py(&odd).contains("requests.request(\"PURGE\", \"https://h/cache\")"));
    }

    #[test]
    fn quotes_backslashes_and_newlines_are_escaped() {
        let state = PersistedState {
            method: "PUT".into(),
            url: "https://h/it's".into(),
            headers_text: "X-Note: say \"hi\" \\ there".into(),
            body_mode: BodyMode::Raw,
            raw_body: "line one\nline \"two\"".into(),
            ..Default::default()
        };
        let text = py(&state);
        assert!(text.contains(r#""X-Note": "say \"hi\" \\ there""#), "{text}");
        assert!(text.contains(r#"data="line one\nline \"two\"""#), "{text}");
    }
}
