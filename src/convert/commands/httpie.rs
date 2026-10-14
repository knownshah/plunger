//! HTTPie: an `http` command. Export only.
//!
//! Syntax: `http [options] METHOD URL Header:value key=value key:=json name@path`
//! - multipart text fields use `=`, files use `@path`
//! - `--form` for url-encoded bodies, `--multipart` for multipart bodies
//! - `--verify=no` when TLS verification is off
//! - a free-form JSON or raw body is passed with `--raw` so `{{variables}}` stay intact

use super::{PartBody, Parts};

/// POSIX-shell single quoting: `it's` -> `'it'\''s'`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Shell quoting does not stop HTTPie from interpreting its own item separators.
fn item_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if matches!(c, ':' | ';' | '=' | '@') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn item_value(value: &str) -> String {
    // Otherwise `key=@text`, `key==text` or `Header:=text` selects a different operator.
    if value.starts_with(['@', '=']) {
        format!("\\{value}")
    } else {
        value.to_string()
    }
}

pub fn export(p: &Parts) -> String {
    // Exported requests must not inherit a body from a pipe or redirected stdin.
    let mut args: Vec<String> = vec!["http".into(), "--ignore-stdin".into()];

    if p.insecure_tls {
        args.push("--verify=no".into());
    }
    if p.follow_redirects {
        args.push("--follow".into());
    }

    match &p.body {
        PartBody::Form(_) => args.push("--form".into()),
        PartBody::Multipart(_) => args.push("--multipart".into()),
        _ => {}
    }

    let mut items = Vec::new();
    for (k, v) in &p.headers {
        // HTTPie must generate the multipart boundary, just as Plunger does when sending.
        if matches!(p.body, PartBody::Multipart(_)) && k.eq_ignore_ascii_case("content-type") {
            continue;
        }
        let name = item_name(k);
        let item = if v.is_empty() {
            format!("{name};")
        } else {
            format!("{name}:{}", item_value(v))
        };
        items.push(quote(&item));
    }

    match &p.body {
        PartBody::None => {}
        PartBody::Text(text) => {
            // Opaque body (JSON or raw). Keep the exact text so placeholders survive.
            args.push(format!("--raw={}", quote(text)));
            if !p
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                // HTTPie otherwise labels a nonempty raw body as JSON.
                items.push(quote("Content-Type:"));
            }
        }
        PartBody::Form(joined) => {
            // These bytes are already encoded. HTTPie's key=value items would encode them again.
            args.push(format!("--raw={}", quote(joined)));
        }
        PartBody::Multipart(fields) => {
            for (name, is_file, value) in fields {
                let name = item_name(name);
                let item = if *is_file {
                    format!("{name}@{value}")
                } else {
                    format!("{name}={}", item_value(value))
                };
                items.push(quote(&item));
            }
        }
    }

    // A field or header beginning with `--` is data, not an HTTPie option.
    args.push("--".into());
    // Empty forms imply GET, and a bare hostname can be mistaken for a method.
    args.push(quote(&p.method));
    args.push(quote(&p.url));
    args.extend(items);
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
    fn a_get_has_an_explicit_method_and_ignores_stdin() {
        let state = PersistedState {
            method: "get".into(),
            url: " https://h/items?page=2 ".into(),
            follow_redirects: false,
            ..Default::default()
        };
        assert_eq!(
            args(&state),
            [
                "http",
                "--ignore-stdin",
                "--",
                "GET",
                "https://h/items?page=2"
            ]
        );
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
        assert!(args(&state).contains(&"POST".into()));
        assert!(
            text.contains("{{token}}") && text.contains("Authorization"),
            "{text}"
        );
        assert!(text.contains("--raw"), "{text}");
        assert!(text.contains("widget"), "{text}");
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
    fn a_form_body_uses_form_flag_and_raw_encoded_data() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/form".into(),
            body_mode: BodyMode::UrlEncoded,
            urlencoded_body: "a=1\nb=two%20words".into(),
            ..Default::default()
        };
        let text = httpie(&state);
        assert!(text.contains("--form"), "{text}");
        assert!(args(&state).contains(&"--raw=a=1&b=two%20words".into()));
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
        assert!(text.contains("--multipart"), "{text}");
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
        assert!(
            text.contains("{{id}}") && text.contains("{{token}}"),
            "{text}"
        );
    }

    fn args(state: &PersistedState) -> Vec<String> {
        shell_words::split(&httpie(state)).unwrap()
    }

    #[test]
    fn encoded_forms_are_sent_verbatim_with_their_content_type() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/form".into(),
            headers_text: "content-type: application/x-www-form-urlencoded; charset=iso-8859-1"
                .into(),
            body_mode: BodyMode::UrlEncoded,
            urlencoded_body:
                "a=two%20words&plus=%2B+%25\na=again&bare&empty=&byte=%FF&token={{token}}".into(),
            ..Default::default()
        };
        let args = args(&state);
        assert!(args.contains(&"--form".into()));
        assert!(args.contains(
            &"--raw=a=two%20words&plus=%2B+%25&a=again&bare&empty=&byte=%FF&token={{token}}".into()
        ));
        assert!(args.contains(
            &"content-type:application/x-www-form-urlencoded; charset=iso-8859-1".into()
        ));
    }

    #[test]
    fn methods_do_not_depend_on_body_contents_or_url_shape() {
        for mode in [
            BodyMode::None,
            BodyMode::Json,
            BodyMode::Raw,
            BodyMode::UrlEncoded,
            BodyMode::Multipart,
        ] {
            for method in ["GET", "POST", "DELETE"] {
                let state = PersistedState {
                    method: method.into(),
                    url: "localhost".into(),
                    body_mode: mode,
                    ..Default::default()
                };
                let args = args(&state);
                let url = args.iter().position(|a| a == "localhost").unwrap();
                assert_eq!(args[url - 1], method, "{mode:?}: {args:?}");
                assert!(args.contains(&"--ignore-stdin".into()), "{args:?}");
            }
        }
    }

    #[test]
    fn text_only_multipart_keeps_its_encoding_and_literal_fields() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/form".into(),
            headers_text: "Content-Type: multipart/form-data; boundary=stale".into(),
            body_mode: BodyMode::Multipart,
            multipart_fields: vec![
                FormField {
                    key: "a:b=@;".into(),
                    kind: FieldKind::Text,
                    value: "@literal {{token}}".into(),
                    enabled: true,
                },
                FormField {
                    key: "equals".into(),
                    kind: FieldKind::Text,
                    value: "=literal".into(),
                    enabled: true,
                },
            ],
            ..Default::default()
        };
        let args = args(&state);
        assert!(args.contains(&"--multipart".into()));
        assert!(args.contains(&r"a\:b\=\@\;=\@literal {{token}}".into()));
        assert!(args.contains(&r"equals=\=literal".into()));
        assert!(!args.iter().any(|a| a.contains("boundary=stale")));
    }

    #[test]
    fn shell_arguments_preserve_opaque_payloads_and_placeholders() {
        let body = "--literal\nO'Brien \\\"quotes\\\" $(echo nope) `nope` $HOME {{value}}";
        for mode in [BodyMode::Json, BodyMode::Raw] {
            let state = PersistedState {
                method: "PUT".into(),
                url: "{{base}}/it's?q=$HOME&v={{query}}".into(),
                headers_text: "X-Note: O'Brien $HOME {{header}}".into(),
                body_mode: mode,
                json_body: body.into(),
                raw_body: body.into(),
                ..Default::default()
            };
            let args = args(&state);
            assert!(args.contains(&state.url));
            assert!(args.contains(&"X-Note:O'Brien $HOME {{header}}".into()));
            assert!(args.contains(&format!("--raw={body}")), "{args:?}");
            assert!(args.contains(&if mode == BodyMode::Json {
                "Content-Type:application/json".into()
            } else {
                "Content-Type:".into()
            }));
        }
    }

    #[test]
    fn empty_and_operator_prefixed_headers_are_literal_headers() {
        let state = PersistedState {
            url: "https://h".into(),
            headers_text: "X-Empty:\nX-At: @literal\nX-Equals: =literal\nX;Name: value".into(),
            ..Default::default()
        };
        let args = args(&state);
        for header in [
            "X-Empty;",
            r"X-At:\@literal",
            r"X-Equals:\=literal",
            r"X\;Name:value",
        ] {
            assert!(args.contains(&header.into()), "{args:?}");
        }
    }

    #[test]
    fn json_values_and_templates_are_not_reserialized() {
        for body in [
            r#"{ "s": "O'Brien", "n": 9007199254740993, "b": true, "a": [null], "dup": 1, "dup": 2 }"#,
            r#"["{{name}}", 1, false, null]"#,
            r#"{"labels": {{labels}}, "optional": "{{who?}}", "default": "{{x|fallback}}"}"#,
        ] {
            let state = PersistedState {
                method: "POST".into(),
                url: "https://h".into(),
                body_mode: BodyMode::Json,
                json_body: body.into(),
                ..Default::default()
            };
            assert!(args(&state).contains(&format!("--raw={body}")));
        }
    }

    #[test]
    fn multipart_file_arguments_preserve_paths_and_placeholder_names() {
        let path = r"C:\data\O'Brien {{file}}.txt";
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h/upload".into(),
            body_mode: BodyMode::Multipart,
            multipart_fields: vec![FormField {
                key: "{{field}}@file".into(),
                kind: FieldKind::File,
                value: path.into(),
                enabled: true,
            }],
            ..Default::default()
        };
        assert!(args(&state).contains(&format!(r"{{{{field}}}}\@file@{path}")));
    }

    #[test]
    fn disabled_tls_and_redirect_options_do_not_add_flags() {
        let state = PersistedState {
            url: "https://h".into(),
            follow_redirects: false,
            insecure_tls: false,
            ..Default::default()
        };
        let args = args(&state);
        assert!(!args.contains(&"--verify=no".into()));
        assert!(!args.contains(&"--follow".into()));
    }

    #[test]
    fn positional_items_cannot_be_interpreted_as_options() {
        let state = PersistedState {
            method: "POST".into(),
            url: "https://h".into(),
            headers_text: "--X-Header: value".into(),
            body_mode: BodyMode::Multipart,
            multipart_fields: vec![FormField {
                key: "--field".into(),
                kind: FieldKind::Text,
                value: "text".into(),
                enabled: true,
            }],
            ..Default::default()
        };
        let args = args(&state);
        let delimiter = args
            .iter()
            .position(|a| a == "--")
            .expect("end HTTPie options before positional arguments");
        assert!(args[..delimiter].contains(&"--multipart".into()));
        assert_eq!(
            &args[delimiter + 1..],
            ["POST", "https://h", "--X-Header:value", "--field=text"]
        );

        let raw = PersistedState {
            body_mode: BodyMode::Raw,
            raw_body: "--literal".into(),
            ..state
        };
        let args = self::args(&raw);
        let delimiter = args.iter().position(|a| a == "--").unwrap();
        assert!(args[..delimiter].contains(&"--raw=--literal".into()));
    }
}
