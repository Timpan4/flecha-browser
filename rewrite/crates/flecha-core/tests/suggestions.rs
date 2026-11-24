use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use flecha_core::suggestions::{
    EnvironmentTone, InputResolution, MAX_SUGGESTIONS, MatchQuality, SuggestionAction,
    describe_environment, developer_suggestions, format_targets, match_quality, parse_suggestions,
    parse_targets, resolve_input, suggestable,
};
use flecha_core::{Environment, SearchEngine};
use serde_json::{Value, json};

#[test]
fn whitespace_and_format_characters_around_credential_separator_stay_local() {
    let engine = SearchEngine::Google;
    for userinfo in ["user", "user:pass"] {
        for separator in [
            "@\u{FEFF}",
            "@\u{FEFF}\t",
            "@\t\u{FEFF}",
            "@\u{FEFF}\u{200A}",
            "@\u{FEFF}\u{00A0}",
            "@\u{FEFF}\u{3000}",
            "@\u{200A}",
            "@\u{2009}",
            "@\u{00A0}",
            "@\u{3000}",
            "@\u{202F}",
            "@\u{2028}",
            "@\t",
            "@ ",
            "@\u{85}\u{200B}",
            "\u{FEFF}\u{2009}@\u{2060}\u{202F}",
        ] {
            for host in ["host.example", "localhost", "127.0.0.1", "[::1]"] {
                for suffix in ["", ":8080", "/path", ":8080/path"] {
                    let input = format!("{userinfo}{separator}{host}{suffix}");
                    assert_eq!(
                        resolve_input(&input, engine),
                        InputResolution::Invalid,
                        "{input:?}"
                    );
                    assert!(engine.suggestion_url(&input).is_none(), "{input:?}");
                }
            }
        }
    }
    for separator in ["@ ", " @", "@\u{feff}"] {
        let input = format!("user{separator}intranet:8080");
        assert_eq!(resolve_input(&input, engine), InputResolution::Invalid);
        assert!(engine.suggestion_url(&input).is_none());
    }
}

#[test]
fn ordinary_at_sign_phrases_still_search() {
    let engine = SearchEngine::Google;
    for query in [
        "rust @ 2026",
        "dinner @ 7pm",
        "lunch @ 12",
        "price @ 5%",
        "tweet @ someone",
        "meet @ noon tomorrow",
        "contact@ example",
        "weather @ stockholm",
    ] {
        assert_eq!(
            resolve_input(query, engine),
            InputResolution::Search(engine.search_url(query)),
            "{query:?}"
        );
    }
}

#[test]
fn developer_aliases_do_not_send_rejected_arguments_to_either_search() {
    let engine = SearchEngine::Google;
    for query in [
        "mdn user:pass@\u{feff}host.example",
        "mdn user:pass@host.example",
        "mdn user@ \u{feff}localhost",
        "mdn https:///private",
        "so mdn user@ host.example",
    ] {
        assert!(
            developer_suggestions(query, None, &[], 0).is_empty(),
            "{query:?}"
        );
        assert_eq!(
            resolve_input(query, engine),
            InputResolution::Invalid,
            "{query:?}"
        );
        assert!(engine.suggestion_url(query).is_none(), "{query:?}");
    }
}

#[test]
fn obscured_explicit_addresses_and_nel_in_hosts_stay_local() {
    let engine = SearchEngine::Google;
    for input in [
        "host.example\u{85}/p?token=x",
        "ho\u{85}st.example/p?token=x",
        "\u{200b}https://host.example/p?token=x",
        "ht\u{ad}tps://host.example/p?token=x",
        "\u{200b}ht\u{feff}tps:///p?token=x",
        "\u{2060}file:///home/u/secret",
    ] {
        assert_eq!(
            resolve_input(input, engine),
            InputResolution::Invalid,
            "{input:?}"
        );
        assert!(engine.suggestion_url(input).is_none(), "{input:?}");
    }
}

#[test]
fn explicit_http_addresses_without_hosts_stay_local() {
    let engine = SearchEngine::Google;
    let inputs = [
        "https://",
        "https:///path?token=x#private",
        "http://:80/x",
        "https://\t/path?token=x",
        "https://\n\r/path#private",
        "https://\t\r\n?token=x",
        "ht\ttps:///p?token=x",
    ];
    assert_eq!(
        inputs.map(|input| resolve_input(input, engine)),
        inputs.map(|_| InputResolution::Invalid)
    );
    for input in inputs {
        assert!(engine.suggestion_url(input).is_none());
    }
}

#[test]
fn command_input_trims_ecmascript_whitespace_and_line_terminators() {
    let engine = SearchEngine::Google;
    let url = "https://host.example/p?token=x";
    let whitespace = [
        '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{a0}', '\u{1680}', '\u{2000}', '\u{2001}',
        '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}',
        '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}',
        '\u{feff}',
    ];
    for ch in whitespace {
        let wrapped = format!("{ch}{url}{ch}");
        assert_eq!(
            resolve_input(&wrapped, engine),
            InputResolution::Navigate(url.into()),
            "U+{:04X}",
            u32::from(ch)
        );
        assert!(engine.suggestion_url(&wrapped).is_none());
        assert_eq!(
            resolve_input(&format!("{ch}https:///p?token=x"), engine),
            InputResolution::Invalid,
            "U+{:04X}",
            u32::from(ch),
        );
    }
    // NEL is Unicode White_Space but isn't trimmed by ECMAScript.
    let with_nel = "\u{85}weather tomorrow\u{85}";
    assert_eq!(
        resolve_input(with_nel, engine),
        InputResolution::Search(engine.search_url(with_nel))
    );
}

#[test]
fn explicit_addresses_keep_non_http_authorities_and_strip_http_controls() {
    let engine = SearchEngine::Google;
    for (input, target) in [
        ("app:///settings", "app:///settings"),
        ("file:///x", "file:///x"),
        ("https://ex\tample.org/a", "https://example.org/a"),
    ] {
        assert_eq!(
            resolve_input(input, engine),
            InputResolution::Navigate(target.into())
        );
    }
    let query = "weather tomorrow";
    assert_eq!(
        resolve_input(query, engine),
        InputResolution::Search(engine.search_url(query))
    );
}

#[test]
fn parses_each_engine_response_and_filters_unsuitable_rows() {
    let google = r#"["githu",["github","https://github.com/","github copilot"],["","GitHub",""],[],{"google:suggesttype":["QUERY","NAVIGATION","QUERY"]}]"#;
    let calculator = r#"["1+1",["= 2","1+1 math"],["Calculator",""],[],{"google:suggesttype":["CALCULATOR","QUERY"]}]"#;
    let bing = r#"["weather",["weather","weather stockholm"],[],[],{"google:suggestrelevance":[1300,1299]}]"#;
    let list = r#"["weather",["weather radar","Weather Radar","weather online"]]"#;
    assert_eq!(
        parse_suggestions(google, "githu"),
        ["github", "github copilot"]
    );
    assert_eq!(parse_suggestions(calculator, "1+1"), ["1+1 math"]);
    assert_eq!(parse_suggestions(bing, "weather"), ["weather stockholm"]);
    assert_eq!(
        parse_suggestions(list, "weather"),
        ["weather radar", "weather online"]
    );
    assert_eq!(parse_suggestions(")]}'\n[\"a\",[\"ab\"]]", "a"), ["ab"]);
    assert_eq!(
        parse_suggestions(r#"["a",[1,null,{"q":"x"},"ab"],null,null,"meta"]"#, "a"),
        ["ab"]
    );
    for body in [
        r#"["weathe",["weather"]]"#,
        "not json",
        r#"{"a":1}"#,
        r#"["a","b"]"#,
    ] {
        assert!(parse_suggestions(body, "a").is_empty(), "{body}");
    }
    let entries: Vec<_> = (0..15).map(|i| format!("q {i}")).collect();
    assert_eq!(
        parse_suggestions(&json!(["q", entries]).to_string(), "q").len(),
        MAX_SUGGESTIONS
    );
    for engine in [
        SearchEngine::Google,
        SearchEngine::DuckDuckGo,
        SearchEngine::Bing,
        SearchEngine::Brave,
        SearchEngine::Kagi,
        SearchEngine::Ecosia,
    ] {
        let url = engine.suggestion_url("café").unwrap();
        assert!(url.starts_with("https://") && url.contains("caf%C3%A9"));
        assert_eq!(engine.suggestion_url("https://example.test/private"), None);
    }
    assert_eq!(
        SearchEngine::DuckDuckGo.search_url("github.com"),
        "https://duckduckgo.com/?q=github.com"
    );
    assert_eq!(
        resolve_input("github.com", SearchEngine::DuckDuckGo),
        InputResolution::Navigate("https://github.com".into())
    );
}

#[test]
fn suggestion_requests_keep_private_input_local() {
    for (query, expected) in [
        ("github.com", false),
        ("https://example.com/a?b", false),
        ("localhost:3000", false),
        ("chrome://settings", false),
        ("about:blank", false),
        ("file:///etc/passwd", false),
        ("javascript:alert(1)", false),
        ("mailto:a@b.c", false),
        ("/home/me/notes.txt", false),
        ("~/notes", false),
        ("./x", false),
        ("\\\\server\\share", false),
        ("C:\\Users\\me", false),
        ("  ", false),
        ("example.com?token=secret", false),
        ("example.com#section", false),
        ("example.com:8443", false),
        ("me@example.com", false),
        ("user:password@example.com", false),
        ("192.168.1.10/private", false),
        ("192.168.1.10", false),
        ("10.0.0.1:8080", false),
        ("[::1]:3000", false),
        ("intranet.example.local/wiki", false),
        ("bücher.de/konto", false),
        ("chrome:", false),
        ("view-source:example.com", false),
        ("example.com.?token=secret", false),
        ("127.1/private?token=secret", false),
        ("0x7f.1", false),
        ("example。com?token=secret", false),
        ("intranet/wiki", false),
        ("intranet:8080", false),
        ("example.com?token=secret more words", false),
        ("me@example.com hello", false),
        ("mailto:someone", false),
        ("example.com/private/.. hello", false),
        ("example.com:80 hello", false),
        ("example.com:080 hello", false),
        ("example.com/%2e%2e hello", false),
        ("[::1]:80 hello", false),
        ("localhost:80 hello", false),
        ("intranet/private/..", false),
        ("intranet/%2e%2e", false),
        ("intranet\\private\\..", false),
        ("intranet/", false),
        ("example.com\n/private?token=secret", false),
        ("example.com\t/private", false),
        ("notes user:secret@host hello", false),
        ("user:secret@host hello", false),
        ("https:example.com/private?token=secret words", false),
        ("see example.com?token=secret", false),
        ("site:example.com cats", false),
        ("chrome:", false),
        ("%65xample%2ecom", false),
        ("%65xample%2ecom/private", false),
        ("notes 127.0.0.1:8080", false),
        ("notes 127.1:80", false),
        ("notes 127.0.0.1/80", false),
        ("notes %65xample%2ecom%3Fq", false),
        ("see 10.0.0.1:22 now", false),
        ("notes 127.0.0.1", false),
        ("notes 127。0。0。1", false),
        ("notes 0x7f.0.0.1", false),
        ("2130706433:80", false),
        ("notes 2130706433:80", false),
        ("notes 2130706433/80", false),
        ("notes 017700000001:80", false),
        ("notes intranet/", false),
        ("notes example.com/", false),
        ("notes example.com?", false),
        ("notes example.com#", false),
        ("notes chrome:", false),
        ("chrome: notes", false),
        ("notes file:", false),
        ("notes about:", false),
        ("notes 127.0.0.1.", false),
        ("see (127.0.0.1).", false),
        ("notes 127.0.0.1,", false),
        ("notes 2130706433?", false),
        ("2130706433#", false),
        ("notes 0x7f000001#", false),
        ("notes chrome-search:", false),
        ("notes isolated-app:", false),
        ("notes chrome-distiller:", false),
        ("\"chrome:\" notes", false),
        ("see example.com/.", false),
        ("notes [::1].", false),
        ("notes 127。0。0。1。", false),
        ("notes 127.0.0.1。", false),
        ("notes 127｡0｡0｡1｡", false),
        ("notes 0x7f。0。0。1。", false),
        ("notes 0177。0。0。1。", false),
        ("notes 127.0.0.1.。", false),
        ("notes 127。0。0。1。、", false),
        ("notes 127。0。0。1。」", false),
        ("notes 127.0.0.1.』", false),
        ("notes 「127。0。0。1。」", false),
        ("サーバー「127.0.0.1」です", false),
        ("notes 🙂127.0.0.1🙂", false),
        ("notes «127.0.0.1»", false),
        ("「example.com」", false),
        ("notes (127.\u{200B}0.0.1)", false),
        ("example.\u{200B}com", false),
        ("127.\u{200B}1", false),
        ("example\u{AD}.com", false),
        ("❤ example.com", false),
        ("🙂 example.com", false),
        ("👍🏽 example.com", false),
        ("「こんにちは」 世界", true),
        ("サーバー「example.com」は遅い", true),
        ("notes «quoted» 🙂", true),
        ("-- done.", true),
        ("! e.g.", true),
        ("... U.S.A.", true),
        ("❤️ done.", true),
        ("notes 1984?", true),
        ("what is 42?", true),
        ("e.g. this", true),
        ("the U.S.A. today", true),
        ("version 1.2.3.", true),
        ("notes 今日は晴れ。", true),
        ("all done.", true),
        ("\"quoted words\"", true),
        ("(parenthetical note)", true),
        ("notes 127.1", true),
        ("version 1.2.3", true),
        ("2026/10/26 events", true),
        ("08:30 standup", true),
        ("re: notes", true),
        ("is it down?", true),
        ("ratio 4:3", true),
        ("1984", true),
        ("2130706433", true),
        ("0x80070005", true),
        ("0306406152", true),
        ("meeting 1/2/2026", true),
        ("at 10:30:15", true),
        ("100%", true),
        ("50%25 off", true),
        ("how to: bake bread", true),
        ("re: meeting notes", true),
        ("c++ vector", true),
        ("c++", true),
        ("what is 3.5 / 2", true),
        ("what is example.com", true),
        ("example.com is down", true),
        ("weather?", true),
        ("C#", true),
        ("C# generics", true),
        ("1/2 cup", true),
        ("10:30 meeting", true),
        ("16:9 monitor", true),
        ("@handle posts", true),
        ("#hashtag", true),
    ] {
        assert_eq!(suggestable(query), expected, "{query:?}");
    }
}

#[test]
fn developer_aliases_and_local_decoders() {
    let now = 1_700_000_000_000;
    let current = Some("https://github.com/team/project/tree/main");
    for (query, url) in [
        ("gh team/project", "https://github.com/team/project"),
        ("gh #42", "https://github.com/team/project/pull/42"),
        (
            "gh cross platform",
            "https://github.com/search?q=cross%20platform",
        ),
        (
            "npm @team/package",
            "https://www.npmjs.com/package/@team/package",
        ),
        (
            "npm build tools",
            "https://www.npmjs.com/search?q=build%20tools",
        ),
        (
            "MDN fetch()",
            "https://developer.mozilla.org/en-US/search?q=fetch()",
        ),
        ("caniuse CSS grid", "https://caniuse.com/?search=CSS%20grid"),
        (
            "so cargo test",
            "https://stackoverflow.com/search?q=cargo%20test",
        ),
    ] {
        assert_eq!(
            developer_suggestions(query, current, &[], now)[0].action,
            SuggestionAction::Open(url.into())
        );
    }
    for query in [
        "constructor hello",
        "__proto__ hello",
        "170000000000",
        "1700000000x",
    ] {
        assert!(developer_suggestions(query, current, &[], now).is_empty());
    }
    let header = json!({ "alg": "none" });
    let payload = json!({ "sub": "räksmörgås", "exp": 1_700_000_060 });
    let h = URL_SAFE_NO_PAD.encode(header.to_string());
    let p = URL_SAFE_NO_PAD.encode(payload.to_string());
    let jwt = format!("{h}.{p}.");
    let result = &developer_suggestions(&jwt, current, &[], now)[0];
    assert_eq!(
        result.label,
        "JWT · alg none · sub räksmörgås · expires in 1m"
    );
    let SuggestionAction::Copy(ref output) = result.action else {
        panic!("expected a copy action")
    };
    assert_eq!(
        serde_json::from_str::<Value>(output).unwrap(),
        json!({ "header": header, "payload": payload })
    );
    assert!(!suggestable(&jwt));
    let malformed = URL_SAFE_NO_PAD.encode(b"{\"sub\":\"\xff\"}");
    assert!(developer_suggestions(&format!("{h}.{malformed}."), current, &[], now).is_empty());
    for (query, expected) in [
        ("1700000000", "2023-11-14T22:13:20.000Z"),
        ("1700000000123", "2023-11-14T22:13:20.123Z"),
    ] {
        assert_eq!(
            developer_suggestions(query, current, &[], now)[0].action,
            SuggestionAction::Copy(expected.into())
        );
    }
}

#[test]
fn environment_switches_keep_the_location_and_follow_target_order() {
    use EnvironmentTone::{Local, Production, Staging};
    let input = "prod https://staging.example.com\nhttp://app.localhost:3030\nhttps://flecha.dev\nhttps://latest.example.com\nhttp://[::1]:8080\nhttp://0.0.0.0:8000\nhttps://staging2.example.com\nhttps://qa.appspot.com\nhttps://qa.project.appspot.com";
    let environments = [Environment {
        id: "project".into(),
        name: "Project".into(),
        targets: parse_targets(input),
    }];
    assert_eq!(format_targets(&environments[0].targets), input);
    assert!(describe_environment("blob:https://flecha.dev/fixture", &environments).is_none());
    let tones: Vec<_> = environments[0]
        .targets
        .iter()
        .map(|target| {
            describe_environment(&target.origin, &environments)
                .unwrap()
                .tone
        })
        .collect();
    assert_eq!(
        tones,
        [
            Production, Local, Production, Production, Local, Local, Staging, Production, Staging
        ]
    );
    let results = developer_suggestions(
        "ENV",
        Some("https://flecha.dev/docs?q=example#section"),
        &environments,
        0,
    );
    assert_eq!(
        results.first().unwrap().action,
        SuggestionAction::Open("https://latest.example.com/docs?q=example#section".into())
    );
    assert_eq!(
        results.last().unwrap().action,
        SuggestionAction::Open("http://app.localhost:3030/docs?q=example#section".into())
    );
    assert_eq!(
        parse_targets("203.0.113.9:3000\n192.168.1.010:3000\ntel:1234\nssh:22\nInternal intranet")
            .into_iter()
            .map(|target| target.origin)
            .collect::<Vec<_>>(),
        ["https://203.0.113.9:3000", "https://intranet"]
    );
}

#[test]
fn match_quality_follows_command_bar_order() {
    for (query, expected) in [
        ("georg", MatchQuality::Start),
        ("okee", MatchQuality::Word),
        ("effe", MatchQuality::Contains),
    ] {
        assert_eq!(match_quality(query, &["Georgia Okeeffe"]), Some(expected));
    }
    assert_eq!(
        match_quality("pin geo", &["Pin to Georgia"]),
        Some(MatchQuality::Word)
    );
}

#[test]
fn jwt_in_a_note_does_not_make_a_suggestion_request() {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#);
    let claims = URL_SAFE_NO_PAD.encode(br#"{"sub":"synthetic"}"#);
    let token = format!("{header}.{claims}.c2lnbmF0dXJl");
    for input in [
        format!("inspect {token} please"),
        format!("inspect ({token})."),
        format!("inspect [{header}%2E{claims}%2Ec2lnbmF0dXJl] please"),
    ] {
        assert!(SearchEngine::Google.suggestion_url(&input).is_none());
    }
}

#[test]
fn github_issue_lookup_never_uses_a_prefix_of_a_repository_name() {
    let suggestion = developer_suggestions(
        "gh 42",
        Some("https://github.com/team/project%2Dtools"),
        &[],
        0,
    );
    assert_eq!(
        suggestion[0].action,
        SuggestionAction::Open("https://github.com/search?q=42".into())
    );
}

#[test]
fn padded_whitespace_in_jwt_header_still_keeps_prose_local() {
    let header = URL_SAFE_NO_PAD.encode(br#"{ "alg": "HS256" }"#);
    let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"synthetic"}"#);
    let token = format!("{header}.{payload}.c2lnbmF0dXJl");
    for input in [
        format!("review {token} please"),
        format!("see note.{token} please"),
        format!("see ({header}%2E{payload}%2Ec2lnbmF0dXJl)"),
    ] {
        assert!(SearchEngine::Google.suggestion_url(&input).is_none());
    }
    assert!(matches!(
        developer_suggestions(&token, None, &[], 0)[0].action,
        SuggestionAction::Copy(_)
    ));
}

#[test]
fn jwt_filter_checks_adjacent_characters_and_encoded_forms() {
    use base64::engine::general_purpose::STANDARD;
    let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"synthetic"}"#);
    let raw_headers = [
        r#"{"alg":"HS256"}"#,
        " {\n \"alg\": \"HS256\" }",
        r#"{"alg":"HS256","kid":"¾ÿÿa"}"#,
    ];
    let wrappers = [
        "",
        "x",
        "123",
        "-_",
        "_",
        ".",
        "%",
        "%zz",
        "\"",
        "[",
        "note-",
        "note=",
        "https://example.org/?token=",
    ];
    for header in raw_headers {
        for encoded in [URL_SAFE_NO_PAD.encode(header), STANDARD.encode(header)] {
            let compact = format!("{encoded}.{payload}.c2lnbmF0dXJl");
            let percent = compact
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect::<String>();
            for spelling in [&compact, &percent] {
                for prefix in wrappers {
                    for suffix in wrappers {
                        let input = format!("review {prefix}{spelling}{suffix} please");
                        assert!(
                            SearchEngine::Google.suggestion_url(&input).is_none(),
                            "prefix {prefix:?}, suffix {suffix:?}, percent-encoded: {}",
                            spelling == &percent
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn jwt_decoder_accepts_standard_base64_and_padding() {
    use base64::engine::general_purpose::STANDARD;
    let header = json!({"alg":"HS256", "kid":"¾ÿÿa"});
    let encoded = STANDARD.encode(header.to_string());
    assert!(encoded.contains('+') && encoded.contains('/') && encoded.ends_with("=="));
    let query = format!("{encoded}.e30.signature");
    let rows = developer_suggestions(&query, None, &[], 0);
    let [row] = rows.as_slice() else {
        panic!("expected a decoded JWT")
    };
    let SuggestionAction::Copy(text) = &row.action else {
        panic!("expected decoded JSON")
    };
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap()["header"],
        header
    );
}

#[test]
fn jwt_privacy_and_decoder_require_a_json_object_header() {
    let array = URL_SAFE_NO_PAD.encode(br#"["header"]"#);
    let non_token = format!("{array}.e30.signature");
    assert!(
        SearchEngine::Google
            .suggestion_url(&format!("inspect {non_token} please"))
            .is_some()
    );
    assert!(developer_suggestions(&non_token, None, &[], 0).is_empty());
    assert_eq!(developer_suggestions("e30.e30.x", None, &[], 0).len(), 1);
}

#[test]
fn jwt_decoder_rejects_a_mixed_base64_alphabet() {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE};
    let header = STANDARD.encode(r#"{"alg":"HS256","kid":"¾ÿÿa"}"#);
    assert!(header.contains('+') && header.contains('/'));
    let mixed = header.replacen('+', "-", 1);
    assert!(STANDARD.decode(&mixed).is_err() && URL_SAFE.decode(&mixed).is_err());
    assert!(developer_suggestions(&format!("{mixed}.e30.signature"), None, &[], 0).is_empty());
}

#[test]
fn jwt_privacy_scans_normalized_and_repeatedly_percent_decoded_dots() {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#);
    let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"fixture"}"#);
    for dot in ["．", "%2E", "。", "%252E"] {
        let note = format!("review {header}{dot}{payload}{dot}signature please");
        assert!(
            SearchEngine::Google.suggestion_url(&note).is_none(),
            "delimiter {dot}"
        );
    }
}

#[test]
fn jwt_privacy_decodes_percent_escaped_fullwidth_percent_as_an_escape() {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#);
    let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"fixture"}"#);
    let token = format!("{header}%EF%BC%852E{payload}%EF%BC%852Esignature");
    assert!(!suggestable(&token));
    assert!(!suggestable(&format!("note {token}")));
}
