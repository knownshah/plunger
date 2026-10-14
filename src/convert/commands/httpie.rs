//! HTTPie (`http`, version 3 or later): export only.

use super::{PartBody, Parts};

/// A bash word: bare when it is plain, single-quoted otherwise.
fn word(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@' | '%' | '+' | ',' | '=' | '~'));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `a%20b` and `a+b` as `a b`: HTTPie encodes a form value itself, so it is given the text, not its encoding.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'%'));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn export(p: &Parts) -> String {
    let mut parts: Vec<String> = vec!["http".into()];
    let mut options: Vec<String> = Vec::new();
    if p.follow_redirects {
        options.push("--follow".into());
    }
    if p.insecure_tls {
        options.push("--verify=no".into());
    }
    match &p.body {
        PartBody::Form(_) => options.push("--form".into()),
        PartBody::Multipart(_) => options.push("--multipart".into()),
        _ => {}
    }
    parts.extend(options);
    parts.push(p.method.clone());
    parts.push(word(&p.url));

    // headers are `Name:value`; a Content-Type that --form sets itself is left out
    let sets_type = matches!(p.body, PartBody::Form(_) | PartBody::Multipart(_));
    for (name, value) in &p.headers {
        if sets_type && name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        parts.push(word(&format!("{name}:{value}")));
    }

    match &p.body {
        PartBody::None => {}
        // --raw sends the text as it is, so JSON is not re-encoded and a placeholder stays where it was
        PartBody::Text(text) => {
            parts.push("--raw".into());
            parts.push(word(text));
        }
        PartBody::Form(text) => {
            for pair in text.split('&').filter(|s| !s.is_empty()) {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                parts.push(word(&format!("{}={}", decode(key), decode(value))));
            }
        }
        PartBody::Multipart(fields) => {
            for (name, is_file, value) in fields {
                if *is_file {
                    parts.push(word(&format!("{name}@{value}")));
                } else {
                    parts.push(word(&format!("{name}={value}")));
                }
            }
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::super::{export, Dialect};
    use crate::domain::model::{BodyMode, FieldKind, FormField, PersistedState};

    fn http(state: &PersistedState) -> String {
        export(state, Dialect::HttpieBash)
    }

    #[test]
    fn a_get_has_the_method_the_url_and_headers() {
        let state = PersistedState {
            method: "GET".into(),
            url: "https://h/items?page=2&q=a%20b".into(),
            headers_text: "Accept: application/json\nX-Trace: two words".into(),
            ..Default::default()
        };
        assert_eq!(http(&state), "http --follow GET 'https://h/items?page=2&q=a%20b' Accept:application/json 'X-Trace:two words'");
    }

    #[test]
    fn a_json_body_is_sent_raw_so_it_is_not_re_encoded() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/orders".into(),
            headers_text: "Authorization: Bearer {{token}}".into(),
            body_mode: BodyMode::Json,
            json_body: r#"{"name":"it's"}"#.into(),
            ..Default::default()
        };
        let text = http(&state);
        assert!(text.contains("'Authorization:Bearer {{token}}'") || text.contains("'Authorization:Bearer"), "{text}");
        assert!(text.contains("Content-Type:application/json"), "{text}");
        assert!(text.ends_with(r#"--raw '{"name":"it'\''s"}'"#), "{text}");
    }

    #[test]
    fn forms_files_and_options() {
        let form = PersistedState {
            method: "POST".into(),
            url: "https://h/form".into(),
            body_mode: BodyMode::UrlEncoded,
            urlencoded_body: "a=1\nb=two%20words".into(),
            ..Default::default()
        };
        assert_eq!(http(&form), "http --follow --form POST https://h/form a=1 'b=two words'");

        let upload = PersistedState {
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
        assert_eq!(http(&upload), "http --verify=no --multipart POST https://h/upload 'title=my doc' file@C:/data/a.txt");
    }
}
