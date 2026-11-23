//! File names from Content-Disposition, the page and the URL, against Chromium's
//! `GetSuggestedFilenameImpl` rules.

use flecha_downloads::{NameSources, Platform, generate_file_name};

const ALL: [Platform; 3] = [Platform::Linux, Platform::Mac, Platform::Windows];

fn named(
    url: &str,
    disposition: Option<&str>,
    suggested: Option<&str>,
    platform: Platform,
) -> String {
    generate_file_name(
        NameSources {
            url,
            content_disposition: disposition,
            suggested_name: suggested,
        },
        platform,
    )
    .as_str()
    .to_owned()
}

fn from_url(url: &str) -> String {
    named(url, None, None, Platform::Linux)
}

fn from_header(header: &str) -> String {
    named(
        "https://example.com/from-url.bin",
        Some(header),
        None,
        Platform::Linux,
    )
}

fn suggested(name: &str, platform: Platform) -> String {
    named("https://example.com/", None, Some(name), platform)
}

#[test]
fn a_url_names_the_file_by_its_last_unescaped_segment() {
    assert_eq!(
        from_url("https://example.com/files/report.pdf"),
        "report.pdf"
    );
    assert_eq!(from_url("https://example.com/get?id=1#top"), "get");
    assert_eq!(from_url("https://example.com/my%20file.txt"), "my file.txt");
    assert_eq!(from_url("https://example.com/a.txt;jsessionid=1"), "a.txt");
}

#[test]
fn without_a_name_the_download_is_called_download_never_the_host() {
    assert_eq!(from_url("https://example.com/"), "download");
    assert_eq!(from_url("data:text/plain,hello/world.txt"), "download");
    assert_eq!(from_url("about:blank"), "download");
    assert_eq!(from_url("not a url"), "download");
    for name in ["---", "_-_", ".", "..", "...."] {
        assert_eq!(suggested(name, Platform::Linux), "download", "{name:?}");
    }
}

#[test]
fn content_disposition_wins_and_filename_star_beats_filename() {
    assert_eq!(from_header("attachment; filename=plain.txt"), "plain.txt");
    assert_eq!(
        from_header(
            "attachment; filename=\"fallback.txt\"; filename*=UTF-8''%E2%82%AC%20rates.txt"
        ),
        "€ rates.txt"
    );
    assert_eq!(
        from_header("attachment; filename*=iso-8859-1'en'caf%E9.txt"),
        "café.txt"
    );
    // A charset this port can't convert gives no ext value, so filename is used.
    assert_eq!(
        from_header("attachment; filename*=KOI8-R''%C1.txt; filename=\"fallback.txt\""),
        "fallback.txt"
    );
    assert_eq!(
        named(
            "https://example.com/u.bin",
            Some("attachment; filename=cd.txt"),
            Some("attribute.txt"),
            Platform::Linux
        ),
        "cd.txt"
    );
}

#[test]
fn content_disposition_values_decode_like_chromium() {
    assert_eq!(
        from_header("attachment; filename=\"=?UTF-8?B?w6l0w6kudHh0?=\""),
        "été.txt"
    );
    assert_eq!(
        from_header("attachment; filename=\"=?ISO-8859-1?Q?caf=E9.txt?=\""),
        "café.txt"
    );
    assert_eq!(
        from_header("attachment; filename=caf%C3%A9.txt"),
        "café.txt"
    );
    // Each whitespace delimiter between plain words is kept.
    assert_eq!(
        from_header("attachment; filename=\"two  words.txt\""),
        "two  words.txt"
    );
    assert_eq!(
        from_header("attachment; filename=\"say \\\"hi\\\".txt\""),
        "say _hi_.txt"
    );
    // Without a disposition type every part is a parameter.
    assert_eq!(from_header("filename=no-type.txt"), "no-type.txt");
    // A missing closing quote is forgiven.
    assert_eq!(from_header("attachment; filename=\"open.txt"), "open.txt");
    // A pair without "=" ends the parameters; the URL names the file.
    assert_eq!(
        from_header("attachment; junk; filename=a.txt"),
        "from-url.bin"
    );
    // An unfinished encoded word fails the value.
    assert_eq!(
        from_header("attachment; filename=\"=?UTF-8?B?w6l0\""),
        "from-url.bin"
    );
    // filename* can't be quoted.
    assert_eq!(
        from_header("attachment; filename*=\"UTF-8''x.txt\""),
        "from-url.bin"
    );
}

#[test]
fn a_name_never_climbs_out_of_its_folder() {
    let cases = [
        ("../../etc/passwd", "_.._etc_passwd"),
        ("/etc/passwd", "_etc_passwd"),
        ("..\\..\\windows\\evil.dll", "_.._windows_evil.dll"),
        ("C:\\Windows\\evil.exe", "C__Windows_evil.exe"),
        ("\\\\server\\share\\evil.exe", "__server_share_evil.exe"),
        ("./.hidden", "_.hidden"),
    ];
    for (input, expected) in cases {
        for platform in ALL {
            assert_eq!(
                suggested(input, platform),
                expected,
                "{input:?} on {platform:?}"
            );
        }
    }
    assert_eq!(
        from_url("https://example.com/..%2F..%2Fetc%2Fpasswd"),
        "_.._etc_passwd"
    );
    assert_eq!(from_url("https://example.com/%2E%2E"), "download");
    assert_eq!(
        from_header("attachment; filename=..\\..\\evil.dll"),
        "_.._evil.dll"
    );
    assert_eq!(
        from_header("attachment; filename*=UTF-8''..%2F..%2Fevil.sh"),
        "_.._evil.sh"
    );
}

#[test]
fn control_format_and_reserved_characters_are_replaced() {
    for platform in ALL {
        assert_eq!(suggested("evil\0.txt", platform), "evil_.txt");
        assert_eq!(suggested("a\nb\tc.txt", platform), "a_b_c.txt");
        // U+202E RIGHT-TO-LEFT OVERRIDE would show "invoice\u{202E}fdp.exe" as invoiceexe.pdf.
        assert_eq!(
            suggested("invoice\u{202E}fdp.exe", platform),
            "invoice_fdp.exe"
        );
        assert_eq!(
            suggested("zero\u{200B}width.txt", platform),
            "zero_width.txt"
        );
        assert_eq!(
            suggested("non\u{FFFE}char\u{FDD0}.txt", platform),
            "non_char_.txt"
        );
        assert_eq!(
            suggested("a<b>c:d\"e|f?g*h.txt", platform),
            "a_b_c_d_e_f_g_h.txt"
        );
    }
}

#[test]
fn whitespace_dots_and_tildes_at_the_ends_are_replaced() {
    assert_eq!(suggested(" name.txt ", Platform::Linux), "_name.txt_");
    assert_eq!(suggested("~name.txt~", Platform::Linux), "_name.txt_");
    assert_eq!(suggested("..name.txt..", Platform::Linux), "name.txt");
    // Windows also drops trailing dots and spaces (CreateFile does) and marks each with _.
    assert_eq!(suggested("name.txt. .", Platform::Windows), "name.txt___");
    assert_eq!(suggested(" name.txt ", Platform::Windows), "_name.txt_");
}

#[test]
fn windows_reserved_names_get_a_prefix_only_on_windows() {
    let cases = [
        ("CON", "_CON"),
        ("con.txt", "_con.txt"),
        ("nul", "_nul"),
        ("LPT1.tar.gz", "_LPT1.tar.gz"),
        ("com9.log", "_com9.log"),
        ("clock$", "_clock$"),
        ("desktop.ini", "_desktop.ini"),
        ("Thumbs.db", "_Thumbs.db"),
        ("CONOUT$", "_CONOUT$"),
    ];
    for (input, expected) in cases {
        assert_eq!(suggested(input, Platform::Windows), expected, "{input:?}");
        assert_eq!(suggested(input, Platform::Linux), input, "{input:?}");
    }
    assert_eq!(suggested("console.txt", Platform::Windows), "console.txt");
    assert_eq!(suggested("com10.txt", Platform::Windows), "com10.txt");
}

#[test]
fn windows_renames_shell_integrated_extensions_and_short_name_tildes() {
    assert_eq!(suggested("evil.lnk", Platform::Windows), "evil.download");
    assert_eq!(suggested("evil.URL", Platform::Windows), "evil.download");
    assert_eq!(
        suggested(
            "x.{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            Platform::Windows
        ),
        "x.download"
    );
    assert_eq!(suggested("evil.lnk", Platform::Linux), "evil.lnk");
    // A possible 8.3 short name can't keep its ~ on Windows (CVE-2014-9390).
    assert_eq!(suggested("PROGRA~1", Platform::Windows), "PROGRA_1");
    assert_eq!(suggested("PROGRA~1.TXT", Platform::Windows), "PROGRA_1.TXT");
    assert_eq!(
        suggested("long name~1.txt", Platform::Windows),
        "long name~1.txt"
    );
    assert_eq!(suggested("PROGRA~1", Platform::Linux), "PROGRA~1");
}

#[test]
fn sanitizing_is_idempotent_and_yields_one_component() {
    let inputs = [
        "../../etc/passwd",
        "/etc/passwd",
        "C:\\Windows\\evil.exe",
        "evil\0.txt",
        "invoice\u{202E}fdp.exe",
        " name.txt ",
        "name.txt. .",
        "CON",
        "evil.lnk",
        "PROGRA~1",
        "---",
        "..",
        "report.pdf",
        "€ rates.txt",
    ];
    for platform in ALL {
        for input in inputs {
            let once = suggested(input, platform);
            assert_eq!(
                suggested(&once, platform),
                once,
                "{input:?} on {platform:?}"
            );
            assert!(!once.is_empty() && once != "." && once != "..");
            assert!(!once.contains(['/', '\\', '\0']), "{once:?}");
        }
    }
}
