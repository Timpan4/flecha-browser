//! Rows ported from Chromium 158's own unit tests, so the port is checked against Chromium's
//! expectations rather than ours. Each table names its source file and test.
//!
//! Left out: rows that need a referrer charset (CEF hands the header over as UTF-8 text),
//! rows with bytes that aren't UTF-8 (a Rust `&str` can't hold them), and rows that need a
//! charset this port doesn't decode (EUC-JP, EUC-KR, KSC5601, ISO-8859-7). Those charsets give
//! no name here, so the caller falls back to the URL. From `GenerateFileName`'s table, rows
//! are left out when their answer depends on the MIME type (this port adds no extension) or on
//! a fallback name other than `download`, the only one downloads use
//! (`IDS_DEFAULT_DOWNLOAD_FILENAME`).

use flecha_downloads::{NameSources, Platform, content_disposition_filename, generate_file_name};

fn check(rows: &[(&str, &str)]) {
    for (header, expected) in rows {
        let expected = (!expected.is_empty()).then(|| (*expected).to_owned());
        assert_eq!(
            content_disposition_filename(header),
            expected,
            "header: {header:?}"
        );
    }
}

/// `net/http/http_content_disposition_unittest.cc`, `HttpContentDispositionTest.Filename`.
#[test]
fn http_content_disposition_filename_rows() {
    check(&[
        ("inline; filename=\"abcde.pdf\"", "abcde.pdf"),
        ("inline;filename=\"abcde.pdf\"", "abcde.pdf"),
        ("  inline   ; filename=\"abcde.pdf\"", "abcde.pdf"),
        ("\t\tinline\t\t; filename=\"abcde.pdf\"", "abcde.pdf"),
        ("attachment; filename=abcde.pdf", "abcde.pdf"),
        ("attachment; filename=abc,de.pdf", "abc"),
        ("filename=abcde.pdf", "abcde.pdf"),
        ("filename= abcde.pdf", "abcde.pdf"),
        ("filename =abcde.pdf", "abcde.pdf"),
        ("filename = abcde.pdf", "abcde.pdf"),
        ("filename\t=abcde.pdf", "abcde.pdf"),
        ("filename \t\t  =abcde.pdf", "abcde.pdf"),
        ("inline; filename=\"abc%20de.pdf\"", "abc de.pdf"),
        ("inline; name=\"abcde.pdf\"", ""),
        ("attachment; name=abcde.pdf", ""),
        ("name=abcde.pdf", ""),
        ("filename=\"abcdef.pdf", "abcdef.pdf"),
        ("inline; filename=\"abc  \tde.pdf\"", "abc   de.pdf"),
        (
            "attachment; filename=\"%EC%98%88%EC%88%A0%20%EC%98%88%EC%88%A0.jpg\"",
            "\u{c608}\u{c220} \u{c608}\u{c220}.jpg",
        ),
        (
            "attachment; filename=\"%F0%90%8C%B0%F0%90%8C%B1abc.jpg\"",
            "\u{10330}\u{10331}abc.jpg",
        ),
        (
            "attachment; filename=\"%EC%98%88%EC%88%A0 \t%EC%98%88%EC%88%A0.jpg\"",
            "\u{c608}\u{c220}  \u{c608}\u{c220}.jpg",
        ),
        (
            "attachment; filename=%E5%B2%A1%E3%80%80%E5%B2%A1.txt",
            "\u{5ca1}\u{3000}\u{5ca1}.txt",
        ),
        (
            "attachment; filename==?utf-8?Q?=E8=8A=B8=E8=A1=93_3=2Epng?=",
            "\u{82b8}\u{8853} 3.png",
        ),
        (
            "attachment; filename==?utf-8?Q?=F0=90=8C=B0_3=2Epng?=",
            "\u{10330} 3.png",
        ),
        (
            "inline; filename=\"=?iso88591?Q?caf=e9_=2epng?=\"",
            "caf\u{e9} .png",
        ),
        (
            "inline; filename=\"=?iso88591?Q?caf=E9_?= .png\"",
            "caf\u{e9} .png",
        ),
        ("attachment; filename=caf\u{e9}.png", "caf\u{e9}.png"),
        // Failure cases.
        ("attachment; filename==?iiso88591?Q?caf=EG?=", ""),
        ("attachment; filename==?iso88591?Q?caf=E3?", ""),
        ("attachment; filename==?iso88591?Q?caf=E3?==", ""),
        ("attachment; filename==?iso88591?Q?caf=E3?=?", ""),
        ("attachment; filename==?iso88591?Q?caf=E3?=?=", ""),
        ("attachment; filename==?windows-1252?Q?=63=61=E?=", ""),
        ("attachment; filename=%EC%98%88%EC%88%A", ""),
        ("attachment; filename=%B7%DD%BD%D1.png", ""),
        (
            "attachment; filename==?windows-1252?Q?caf=E3?==?iso-8859-7?b?4eIucG5nCg==?=",
            "",
        ),
        // RFC 5987.
        ("attachment; filename*=foo.html", ""),
        ("attachment; filename*=foo'.html", ""),
        ("attachment; filename*=''foo'.html", ""),
        ("attachment; filename*=''foo.html'", ""),
        ("attachment; filename*=''f\"oo\".html'", ""),
        ("attachment; filename*=bogus_charset''foo.html'", ""),
        ("attachment; filename*='en'foo.html'", ""),
        ("attachment; filename*=iso-8859-1'en'foo.html", "foo.html"),
        ("attachment; filename*=utf-8'en'foo.html", "foo.html"),
        (
            "attachment; filename*=utf-8'en'%E5%B2%A1%E3%80%80%E5%B2%A1.txt",
            "\u{5ca1}\u{3000}\u{5ca1}.txt",
        ),
        ("attachment; filename*=utf-8'es'f\u{3ba}.html", ""),
        ("inline; filename*=iso88591''cafe foo.png", "cafe foo.png"),
        (
            "attachment; filename*=utf-8''%E8%8A%B8%E8%A1%93%203%2Epng",
            "\u{82b8}\u{8853} 3.png",
        ),
        (
            "attachment; filename*=utf-8''%F0%90%8C%B0 3.png",
            "\u{10330} 3.png",
        ),
        (
            "attachment; filename*=windows-1252''caf%E9.png",
            "caf\u{e9}.png",
        ),
        ("attachment; name=\"foo\"; filename=\"bar\"", "bar"),
        ("attachment; filename=\"bar\"; name=\"foo\"", "bar"),
        ("attachment; filename=\"bar\"; filename*=utf-8''baz", "baz"),
        (
            "attachment; filename*=iso-8859-1''foo-%E4.html",
            "foo-\u{e4}.html",
        ),
        (
            "attachment; filename*=UTF-8''foo-%c3%a4-%e2%82%ac.html",
            "foo-\u{e4}-\u{20ac}.html",
        ),
        ("attachment; filename*=''foo-%c3%a4-%e2%82%ac.html", ""),
        // attwithfn2231utf8comp: NFC.
        (
            "attachment; filename*=UTF-8''foo-a%cc%88.html",
            "foo-\u{e4}.html",
        ),
        ("attachment; filename *=UTF-8''foo-%c3%a4.html", ""),
        (
            "attachment; filename*= UTF-8''foo-%c3%a4.html",
            "foo-\u{e4}.html",
        ),
        (
            "attachment; filename* =UTF-8''foo-%c3%a4.html",
            "foo-\u{e4}.html",
        ),
        ("attachment; filename*=\"UTF-8''foo-%c3%a4.html\"", ""),
        (
            "attachment; filename=\"foo-ae.html\"; filename*=UTF-8''foo-%c3%a4.html",
            "foo-\u{e4}.html",
        ),
        (
            "attachment; filename*=UTF-8''foo-%c3%a4.html; filename=\"foo-ae.html\"",
            "foo-\u{e4}.html",
        ),
        ("attachment; foobar=x; filename=\"foo.html\"", "foo.html"),
    ]);
}

/// The same file, `HttpContentDispositionTest.tc2231` (greenbytes.de/tech/tc2231).
#[test]
fn http_content_disposition_tc2231_rows() {
    check(&[
        ("inline", ""),
        ("\"inline\"", ""),
        ("inline; filename=\"foo.html\"", "foo.html"),
        (
            "inline; filename=\"Not an attachment!\"",
            "Not an attachment!",
        ),
        ("inline; filename=\"foo.pdf\"", "foo.pdf"),
        ("attachment", ""),
        ("\"attachment\"", ""),
        ("ATTACHMENT", ""),
        ("attachment; filename=\"foo.html\"", "foo.html"),
        ("attachment; filename=\"f\\oo.html\"", "foo.html"),
        (
            "attachment; filename=\"\\\"quoting\\\" tested.html\"",
            "\"quoting\" tested.html",
        ),
        (
            "attachment; filename=\"Here's a semicolon;.html\"",
            "Here's a semicolon;.html",
        ),
        ("attachment; foo=\"bar\"; filename=\"foo.html\"", "foo.html"),
        (
            "attachment; foo=\"\\\"\\\\\";filename=\"foo.html\"",
            "foo.html",
        ),
        ("attachment; FILENAME=\"foo.html\"", "foo.html"),
        ("attachment; filename=foo.html", "foo.html"),
        ("attachment; filename=foo.html ;", "foo.html"),
        ("attachment; ;filename=foo", "foo"),
        ("attachment; filename=foo bar.html", "foo bar.html"),
        ("attachment; filename='foo.bar'", "'foo.bar'"),
        (
            "attachment; filename=\"foo-\u{e4}.html\"",
            "foo-\u{e4}.html",
        ),
        ("attachment; filename=\"foo-%41.html\"", "foo-A.html"),
        ("attachment; filename=\"50%.html\"", "50%.html"),
        ("attachment; filename=\"foo-%\\41.html\"", "foo-A.html"),
        ("attachment; name=\"foo-%41.html\"", ""),
        (
            "attachment; filename=\"foo-%c3%a4-%e2%82%ac.html\"",
            "foo-\u{e4}-\u{20ac}.html",
        ),
        ("attachment; filename =\"foo.html\"", "foo.html"),
        (
            "attachment; filename=\"foo.html\"; filename=\"bar.html\"",
            "foo.html",
        ),
        ("attachment; filename=foo[1](2).html", "foo[1](2).html"),
        ("attachment; filename=foo-\u{e4}.html", "foo-\u{e4}.html"),
        ("filename=foo.html", "foo.html"),
        ("x=y; filename=foo.html", "foo.html"),
        ("\"foo; filename=bar;baz\"; filename=qux", ""),
        ("filename=foo.html, filename=bar.html", "foo.html"),
        ("; filename=foo.html", "foo.html"),
        ("inline; attachment; filename=foo.html", ""),
        ("attachment; inline; filename=foo.html", ""),
        ("attachment; filename=\"foo.html\".txt", "foo.html\".txt"),
        ("attachment; filename=\"bar", "bar"),
        (
            "attachment; filename=foo\"bar;baz\"qux",
            "foo\"bar;baz\"qux",
        ),
        (
            "attachment; filename=foo.html, attachment; filename=bar.html",
            "foo.html",
        ),
        ("attachment; foo=foo filename=bar", ""),
        ("filename=foo.html; attachment", "foo.html"),
        ("attachment; xfilename=foo.html", ""),
        ("attachment; filename=\"/foo.html\"", "/foo.html"),
        ("attachment; filename=\"\\\\foo.html\"", "\\foo.html"),
        ("foobar", ""),
        ("attachment; example=\"filename=example.txt\"", ""),
        ("attachment; foobar=x; filename=\"foo.html\"", "foo.html"),
    ]);
}

fn generated(url: &str, header: &str, suggested: &str, platform: Platform) -> String {
    generate_file_name(
        NameSources {
            url,
            content_disposition: (!header.is_empty()).then_some(header),
            suggested_name: (!suggested.is_empty()).then_some(suggested),
        },
        platform,
    )
    .as_str()
    .to_owned()
}

/// `net/base/filename_util_unittest.cc`, `FilenameUtilTest.GenerateFileName`, rows whose
/// expectation is the same on every platform: (URL, Content-Disposition, suggested name,
/// expected).
#[test]
fn generate_file_name_rows() {
    let rows: &[(&str, &str, &str, &str)] = &[
        // selection_tests
        (
            "http://www.google.com/",
            "attachment; filename=test.html",
            "",
            "test.html",
        ),
        (
            "http://www.google.com/",
            "attachment; filename=\"test.html\"",
            "",
            "test.html",
        ),
        (
            "http://www.google.com/",
            "attachment; filename= \"test.html\"",
            "",
            "test.html",
        ),
        (
            "http://www.google.com/",
            "attachment; filename   =   \"test.html\"",
            "",
            "test.html",
        ),
        (
            "http://www.google.com/path/test.html",
            "attachment",
            "",
            "test.html",
        ),
        (
            "http://www.google.com/path/test.html",
            "attachment;",
            "",
            "test.html",
        ),
        ("http://www.google.com/test.html", "", "", "test.html"),
        ("http://www.google.com/path", "", "", "path"),
        ("file:///", "", "", "download"),
        ("file:///path/testfile", "", "", "testfile"),
        ("non-standard-scheme:", "", "", "download"),
        (
            "http://www.google.com/",
            "attachment; filename =\"test.html\"",
            "",
            "test.html",
        ),
        ("http://www.google.com/", "", "", "download"),
        (
            "http://www.google.com/",
            "attachment; filename=\"%EC%98%88%EC%88%A0%20%EC%98%88%EC%88%A0.jpg\"",
            "",
            "\u{c608}\u{c220} \u{c608}\u{c220}.jpg",
        ),
        (
            "http://www.google.com/%EC%98%88%EC%88%A0%20%EC%98%88%EC%88%A0.jpg",
            "",
            "",
            "\u{c608}\u{c220} \u{c608}\u{c220}.jpg",
        ),
        (
            "http://www.example.com/images?id=3",
            "attachment; filename=caf\u{e9}.png",
            "",
            "caf\u{e9}.png",
        ),
        (
            "http://www.google.com/test.html",
            "attachment; filename==?iiso88591?Q?caf=EG?=",
            "",
            "test.html",
        ),
        ("about:chrome", "", "", "download"),
        ("data:,looks/like/a.path", "", "", "download"),
        (
            "data:text/plain;base64,VG8gYmUgb3Igbm90IHRvIGJlLg=",
            "",
            "",
            "download",
        ),
        ("http://www.google.com/test", "", "suggested", "suggested"),
        (
            "http://www.google.com/test",
            "attachment; filename=test.html",
            "suggested",
            "test.html",
        ),
        (
            "http://www.google.com/test",
            "attachment; filename=test",
            "",
            "test",
        ),
        (
            "http://www.evil.com/my_download.txt",
            "a_file_name.txt",
            "",
            "my_download.txt",
        ),
        (
            "http://www.frontpagehacker.com/a_download.exe",
            "filename=My Downloaded File.exe",
            "",
            "My Downloaded File.exe",
        ),
        (
            "http://www.examples.com/q.cgi?id=abc",
            "attachment; name=abc de.pdf",
            "",
            "q.cgi",
        ),
        (
            "http://www.example.com/test%20123",
            "attachment; filename==?iiso88591?Q?caf=EG?=",
            "",
            "test 123",
        ),
        (
            "http://www.google.com/%EC%98%88%EC%88%A0%20%EC%98%88%EC%88%A0.jpg",
            "malformed_disposition",
            "",
            "\u{c608}\u{c220} \u{c608}\u{c220}.jpg",
        ),
        (
            "http://www.google.com/path1/path2/",
            "attachment; filename==?iso88591?Q?caf=E3?",
            "",
            "download",
        ),
        // generation_tests
        ("http://www.google.com/.test.html", "", "", "test.html"),
        ("http://www.google.com/.test", "", "", "test"),
        ("http://www.google.com/..test", "", "", "test"),
        (
            "",
            "filename=../../../../././../a_file_name.txt",
            "",
            "_.._.._.._._._.._a_file_name.txt",
        ),
        (
            "",
            "filename=dir1/dir2/a_file_name.txt",
            "",
            "dir1_dir2_a_file_name.txt",
        ),
        (
            "",
            "filename=..\\..\\..\\..\\.\\.\\..\\a_file_name.txt",
            "",
            "_.._.._.._._._.._a_file_name.txt",
        ),
        (
            "",
            "filename=dir1\\dir2\\a_file_name.txt",
            "",
            "dir1_dir2_a_file_name.txt",
        ),
        (
            "",
            "filename=\"<blink>Hello kitty</blink>\"",
            "",
            "_blink_Hello kitty__blink_",
        ),
        ("https://example.com/misc/2.avi", "", "", "2.avi"),
        (
            "http://example.com/foo%2f..%2fbar.jpg",
            "",
            "",
            "foo_.._bar.jpg",
        ),
        (
            "http://example.com/download.py",
            "filename=foo%00bar.jpg",
            "",
            "foo_bar.jpg",
        ),
        ("", "filename=my-cat", "", "my-cat"),
        ("", "filename=my-cat.jpg", "", "my-cat.jpg"),
        ("", "attachment; filename=\"meh.exe\u{a0}\"", "", "meh.exe_"),
        ("", "filename=.", "", "download"),
        ("", "filename=..", "", "download"),
        ("", "filename=...", "", "download"),
        ("", "filename=mycom1.foo", "", "mycom1.foo"),
        ("http://www.example.com/foo.tar.gz", "", "", "foo.tar.gz"),
        ("http://www.example.com/foo.tgz", "", "", "foo.tgz"),
        ("http://maged.lordaeron.org/blank.reg", "", "", "blank.reg"),
        ("http://www.example.com/bar.tar", "", "", "bar.tar"),
        ("http://www.example.com/bar.bogus", "", "", "bar.bogus"),
        ("http://www.example.com/bar.sh", "", "", "bar.sh"),
        (
            "http://www.example.com/npdf.php?fn=foobar.pdf",
            "filename=foobar.jpg",
            "",
            "foobar.jpg",
        ),
        (
            "http://www.example.com/image.aspx?id=blargh",
            "",
            " .foo",
            "_.foo",
        ),
        (
            "http://www.example.com/goat.tar.gz?wearing_hat=true",
            "",
            "",
            "goat.tar.gz",
        ),
        (
            "http://www.example.com/goat.tgz?wearing_hat=true",
            "",
            "",
            "goat.tgz",
        ),
        (
            "http://www.example.com/%E5%B2%A1%E3%80%80%E5%B2%A1.txt",
            "",
            "",
            "\u{5ca1}\u{3000}\u{5ca1}.txt",
        ),
        (
            "http://www.example.com/download.py",
            "filename=%E5%B2%A1%E3%80%80%E5%B2%A1.txt",
            "",
            "\u{5ca1}\u{3000}\u{5ca1}.txt",
        ),
    ];
    for platform in [Platform::Linux, Platform::Mac, Platform::Windows] {
        for (url, header, suggested, expected) in rows {
            assert_eq!(
                generated(url, header, suggested, platform),
                *expected,
                "{url:?} {header:?} {suggested:?} on {platform:?}"
            );
        }
    }
}

/// The same table's rows that differ between POSIX (Linux, macOS) and Windows:
/// (Content-Disposition, suggested name, POSIX, Windows).
#[test]
fn generate_file_name_platform_rows() {
    let rows: &[(&str, &str, &str, &str)] = &[
        ("filename=evil.exe ", "", "evil.exe", "evil.exe"),
        ("filename=evil.exe.", "", "evil.exe", "evil.exe_"),
        (
            "filename=evil.exe.  .  .",
            "",
            "evil.exe.  . _",
            "evil.exe_______",
        ),
        ("filename=evil.", "", "evil", "evil_"),
        ("filename=. . . . .", "", "_. . ._", "download"),
        ("filename=COM1", "", "COM1", "_COM1"),
        ("filename=COM4.txt", "", "COM4.txt", "_COM4.txt"),
        ("filename=lpt1.TXT", "", "lpt1.TXT", "_lpt1.TXT"),
        ("filename=clock$.txt", "", "clock$.txt", "_clock$.txt"),
        ("", "clock$.txt", "clock$.txt", "_clock$.txt"),
        (
            "filename=Setup.exe.local",
            "",
            "Setup.exe.local",
            "Setup.exe.download",
        ),
        (
            "filename=Setup.exe.local.local",
            "",
            "Setup.exe.local.local",
            "Setup.exe.local.download",
        ),
        (
            "filename=Setup.exe.lnk",
            "",
            "Setup.exe.lnk",
            "Setup.exe.download",
        ),
        ("filename=Desktop.ini", "", "Desktop.ini", "_Desktop.ini"),
        ("filename=Thumbs.db", "", "Thumbs.db", "_Thumbs.db"),
    ];
    for (header, suggested, posix, windows) in rows {
        for (platform, expected) in [
            (Platform::Linux, posix),
            (Platform::Mac, posix),
            (Platform::Windows, windows),
        ] {
            assert_eq!(
                generated("", header, suggested, platform),
                *expected,
                "{header:?} {suggested:?} on {platform:?}"
            );
        }
    }
}
