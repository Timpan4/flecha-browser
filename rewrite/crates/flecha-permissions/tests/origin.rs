use flecha_permissions::{
    Evaluation, Origin, OriginError, Partition, PermissionError, PermissionKind, PermissionManager,
    TabId, Verdict,
};

#[test]
fn origins_are_canonical_scheme_host_port() {
    let cases = [
        (
            "HTTPS://Example.COM:443/path?q=1#frag",
            "https://example.com",
        ),
        ("https://user:pass@example.com/", "https://example.com"),
        ("https://example.com:8443/a", "https://example.com:8443"),
        ("http://example.com:80/", "http://example.com"),
        ("https://bücher.example/", "https://xn--bcher-kva.example"),
        ("http://[0:0:0:0:0:0:0:1]:3000/", "http://[::1]:3000"),
    ];
    for (input, expected) in cases {
        assert_eq!(Origin::parse(input).unwrap().as_str(), expected, "{input}");
    }
}

#[test]
fn non_web_and_opaque_urls_have_no_origin() {
    let cases = [
        (
            "data:text/html,hi",
            OriginError::UnsupportedScheme("data".into()),
        ),
        (
            "file:///etc/passwd",
            OriginError::UnsupportedScheme("file".into()),
        ),
        (
            "javascript:alert(1)",
            OriginError::UnsupportedScheme("javascript".into()),
        ),
        (
            "blob:https://example.com/0f1d",
            OriginError::UnsupportedScheme("blob".into()),
        ),
        (
            "wss://example.com/",
            OriginError::UnsupportedScheme("wss".into()),
        ),
        (
            "ftp://example.com/",
            OriginError::UnsupportedScheme("ftp".into()),
        ),
        (
            "chrome://settings",
            OriginError::UnsupportedScheme("chrome".into()),
        ),
        (
            "about:blank",
            OriginError::UnsupportedScheme("about".into()),
        ),
        ("not a url", OriginError::Invalid),
        ("", OriginError::Invalid),
    ];
    for (input, expected) in cases {
        assert_eq!(Origin::parse(input), Err(expected), "{input}");
    }
}

#[test]
fn only_https_and_loopback_http_are_secure_contexts() {
    let trustworthy = [
        "https://example.com",
        "http://localhost:8080",
        "http://LOCALHOST./",
        "http://app.localhost/",
        "http://127.0.0.1/",
        "http://127.8.9.10:5000/",
        "http://[::1]/",
    ];
    for input in trustworthy {
        assert!(
            Origin::parse(input).unwrap().is_potentially_trustworthy(),
            "{input}"
        );
    }
    let insecure = [
        "http://example.com",
        "http://localhost.example.com",
        "http://notlocalhost/",
        "http://10.0.0.1/",
        "http://[::2]/",
        "http://0.0.0.0/",
    ];
    for input in insecure {
        assert!(
            !Origin::parse(input).unwrap().is_potentially_trustworthy(),
            "{input}"
        );
    }
}

#[test]
fn insecure_origins_are_blocked_without_a_prompt() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("t");
    let origin = Origin::parse("http://example.com").unwrap();
    for kind in PermissionKind::ALL {
        assert_eq!(
            manager.evaluate(&Partition::Shared, &tab, &origin, kind),
            Evaluation::Resolved(Verdict::Block),
        );
    }
    assert!(manager.pending_prompt(&tab).is_none());
    assert_eq!(
        manager.set_setting(
            &Partition::Shared,
            &origin,
            PermissionKind::Camera,
            flecha_permissions::Decision::Allow
        ),
        Err(PermissionError::InsecureOrigin),
    );
    assert!(!manager.is_dirty());
}

#[test]
fn loopback_http_can_prompt() {
    let mut manager = PermissionManager::new();
    let tab = TabId::new("t");
    let origin = Origin::parse("http://localhost:3000/app").unwrap();
    assert!(matches!(
        manager.evaluate(&Partition::Shared, &tab, &origin, PermissionKind::Camera),
        Evaluation::Prompt(_)
    ));
}
