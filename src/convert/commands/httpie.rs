//! HTTPie: an `http` command. Export only.
//!
//! Syntax: `http [options] METHOD URL Header:value key=value key:=json name@path`
//! - strings use `=`, non-string JSON uses `:=`, files use `@path`
//! - `--form` for url-encoded / multipart form bodies
//! - `--verify=no` when TLS verification is off
//! - a free-form JSON or raw body is passed with `--raw` so `{{variables}}` stay intact

use super::{PartBody, Parts};

/// POSIX-shell single quoting: `it's` -> `'it'\''s'`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub fn export(p: &Parts) -> String {
    let mut args: Vec<String> = vec!["http".into()];

    if p.insecure_tls {
        args.push("--verify=no".into());
    }
    if p.follow_redirects {
        args.push("--follow".into());
    }

    let form_body = matches!(p.body, PartBody::Form(_) | PartBody::Multipart(_));
    if form_body {
        args.push("--form".into());
    }

    // HTTPie defaults to GET with no data and POST once there is a body; spell the method out
    // only when it differs (same idea as the curl exporters).
    let implied = if p.has_body() { "POST" } else { "GET" };
    if p.method != implied {
        args.push(p.method.clone());
    }

    args.push(quote(&p.url));

    for (k, v) in &p.headers {
        // HTTPie sets Content-Type for --form itself (including the multipart boundary).
        if form_body && k.eq_ignore_ascii_case("content-type") {
            continue;
        }
        args.push(quote(&format!("{k}:{v}")));
    }

    match &p.body {
        PartBody::None => {}
        PartBody::Text(text) => {
            // Opaque body (JSON or raw). Keep the exact text so placeholders survive.
            args.push("--raw".into());
            args.push(quote(text));
        }
        PartBody::Form(joined) => {
            for pair in joined.split('&').filter(|s| !s.is_empty()) {
                args.push(quote(pair));
            }
        }
        PartBody::Multipart(fields) => {
            for (name, is_file, value) in fields {
                let item = if *is_file {
                    format!("{name}@{value}")
                } else {
                    format!("{name}={value}")
                };
                args.push(quote(&item));
            }
        }
    }

    args.join(" \\\n  ")
}

#[cfg(test)]
mod tests {
    use super::super::{export, Dialect};
    use crate::domain::model::{BodyMode, FieldKind, FormField, PersistedState};

    fn httpie(state: &PersistedState) -> String {
        export(state, Dialect::Httpie)
    }

    #[test]
    fn a_get_is_a_bare_http() {
        let state = PersistedState {
            method: "get".into(),
            url: " https://h/items?page=2 ".into(),
            ..Default::default()
        };
        assert_eq!(httpie(&state), "http \\\n  'https://h/items?page=2'");
    }

    #[test]
    fn a_json_post_uses_raw_and_keeps_placeholders() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/orders".into(),
            headers_text: "Authorization: Bearer {{token}}".into(),
            body_mode: BodyMode::Json,
            json_body: r#"{"name":"widget","note":"say \"hi\""}"#.into(),
            ..Default::default()
        };
        let text = httpie(&state);
        assert!(text.starts_with("http \\\n  'https://h/orders'"), "{text}");
        assert!(text.contains("{{token}}") && text.contains("Authorization"), "{text}");
        assert!(text.contains("--raw"), "{text}");
        assert!(text.contains("widget"), "{text}");
        assert!(!text.contains("-X POST") && !text.contains(" POST "), "POST is implied by the body: {text}");
    }

    #[test]
    fn a_raw_body_and_delete_are_spelled_out() {
        let state = PersistedState {
            method: "DELETE".into(),
            url: "https://h/items/1".into(),
            body_mode: BodyMode::Raw,
            raw_body: "plain text, with 'quotes'".into(),
            insecure_tls: true,
            ..Default::default()
        };
        let text = httpie(&state);
        assert!(text.contains("DELETE"), "{text}");
        assert!(text.contains("--verify=no"), "{text}");
        assert!(text.contains("--raw"), "{text}");
        assert!(text.contains("plain text"), "{text}");
    }

    #[test]
    fn a_form_body_uses_form_flag_and_pairs() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/form".into(),
            body_mode: BodyMode::UrlEncoded,
            urlencoded_body: "a=1\nb=two%20words".into(),
            ..Default::default()
        };
        let text = httpie(&state);
        assert!(text.contains("--form"), "{text}");
        assert!(text.contains("'a=1'"), "{text}");
        assert!(text.contains("'b=two%20words'"), "{text}");
    }

    #[test]
    fn a_multipart_body_uses_at_for_files() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/upload".into(),
            body_mode: BodyMode::Multipart,
            multipart_fields: vec![
                FormField {
                    key: "title".into(),
                    kind: FieldKind::Text,
                    value: "my doc".into(),
                    enabled: true,
                },
                FormField {
                    key: "file".into(),
                    kind: FieldKind::File,
                    value: "C:/data/a.txt".into(),
                    enabled: true,
                },
                FormField {
                    key: "off".into(),
                    kind: FieldKind::Text,
                    value: "x".into(),
                    enabled: false,
                },
            ],
            ..Default::default()
        };
        let text = httpie(&state);
        assert!(text.contains("--form"), "{text}");
        assert!(text.contains("'title=my doc'"), "{text}");
        assert!(text.contains("'file@C:/data/a.txt'"), "{text}");
        assert!(!text.contains("off="), "{text}");
    }

    #[test]
    fn redirects_and_insecure_tls_are_flags() {
        let state = PersistedState {
            url: "https://h/x".into(),
            follow_redirects: true,
            insecure_tls: true,
            ..Default::default()
        };
        let text = httpie(&state);
        assert!(text.contains("--follow"), "{text}");
        assert!(text.contains("--verify=no"), "{text}");
    }

    #[test]
    fn variables_are_never_resolved() {
        let state = PersistedState {
            method: "GET".into(),
            url: "https://h/{{id}}".into(),
            headers_text: "Authorization: Bearer {{token}}".into(),
            ..Default::default()
        };
        let text = httpie(&state);
        assert!(text.contains("{{id}}") && text.contains("{{token}}"), "{text}");
    }
}
