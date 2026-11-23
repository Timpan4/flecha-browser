//! Replays `fixtures/boosts-parity.jsonl`, recorded from the Chromium shell's own Boosts code by
//! `scripts/generate-boosts-fixture.ts`.

use std::collections::BTreeMap;

use flecha_boosts::compile::{compile_with, once_per_document};
use flecha_boosts::{
    BoostKind, BoostName, Boosts, CompiledBoost, Effect, Frame, RunAt, SHELL_BUILDS_KEY,
    SHELL_FILES_KEY, ScriptInjection, StyleInjection, applies, boost_host_for, boost_matches,
    user_script_matches,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn rows(kind: &str) -> Vec<Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/boosts-parity.jsonl"
    );
    let text = std::fs::read_to_string(path).expect("read the parity fixture");
    let rows: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("a fixture row"))
        .filter(|row: &Value| row["kind"] == kind)
        .collect();
    assert!(!rows.is_empty(), "no {kind} rows");
    rows
}

fn str_of(value: &Value) -> &str {
    value.as_str().expect("a string")
}

fn is_web(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

#[test]
fn names_parse_as_in_the_shell() {
    for row in rows("name") {
        let name = str_of(&row["name"]);
        let parsed = BoostName::parse(name).map(|n| json!({ "host": n.host(), "kind": n.kind() }));
        assert_eq!(parsed.unwrap_or(Value::Null), row["parsed"], "{name:?}");
    }
}

#[test]
fn pages_match_and_name_hosts_as_in_the_shell() {
    let hosts = rows("url-hosts").remove(0)["hosts"].clone();
    let hosts: Vec<&str> = hosts.as_array().unwrap().iter().map(str_of).collect();
    for row in rows("url") {
        let url = str_of(&row["url"]);
        assert_eq!(
            boost_host_for(Some(url)).map_or(Value::Null, Value::from),
            row["host"],
            "host for {url:?}"
        );
        let matched: Vec<&str> = hosts
            .iter()
            .copied()
            .filter(|host| boost_matches(url, host))
            .collect();
        let expected: Vec<&str> = row["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(str_of)
            .collect();
        assert_eq!(matched, expected, "hosts matching {url:?}");
    }
    assert_eq!(boost_host_for(None), None);
}

/// What the shell asked Chromium to do during one step.
#[derive(Default)]
struct Calls {
    stored: Option<Value>,
    registered: Option<Vec<(String, String)>>,
    styles: BTreeMap<u64, Vec<String>>,
    reloads: Vec<u64>,
    scripts: Vec<(String, String)>,
    /// Tabs where Chromium refused a call, which then had no effect.
    refused: Vec<u64>,
}

fn calls(list: &Value) -> Calls {
    let mut calls = Calls::default();
    let mut warned = false;
    for call in list.as_array().unwrap() {
        let tab = || call["tabId"].as_u64().unwrap();
        if call["refused"] == true {
            calls.refused.push(tab());
            continue;
        }
        match str_of(&call["api"]) {
            "storage.set" => calls.stored = Some(call["items"].clone()),
            "register" => {
                let scripts = call["scripts"].as_array().unwrap();
                for script in scripts {
                    assert_eq!(script["world"], "MAIN");
                    assert_eq!(script["runAt"], "document_end");
                    assert_eq!(script["allFrames"], false);
                    // user_script_matches assumes these patterns.
                    let host = BoostName::parse(str_of(&script["id"])).unwrap();
                    let host = host.host();
                    let patterns = if host == "all" {
                        json!(["http://*/*", "https://*/*"])
                    } else {
                        json!([format!("*://{host}/*"), format!("*://*.{host}/*")])
                    };
                    assert_eq!(script["matches"], patterns);
                }
                assert_eq!(ScriptInjection::RUN_AT, RunAt::DocumentEnd);
                calls.registered = Some(
                    scripts
                        .iter()
                        .map(|s| (str_of(&s["id"]).into(), str_of(&s["code"]).into()))
                        .collect(),
                );
            }
            "insertCSS" => {
                assert_eq!(call["origin"], "USER");
                calls
                    .styles
                    .entry(tab())
                    .or_default()
                    .push(str_of(&call["css"]).into());
            }
            // The app removes what it inserted itself; Boosts says what to insert.
            "removeCSS" => {
                calls.styles.entry(tab()).or_default();
            }
            "reload" => calls.reloads.push(tab()),
            "execute" => {
                assert_eq!(call["world"], "MAIN");
                calls
                    .scripts
                    .push(("run".into(), str_of(&call["code"]).into()));
            }
            "consoleError" => {
                assert_eq!(call["world"], "MAIN");
                calls
                    .scripts
                    .push(("error".into(), str_of(&call["message"]).into()));
            }
            "warn" => warned = true,
            "unregister" => {}
            api => panic!("unexpected call {api}"),
        }
    }
    // The shell warns when Chromium refuses it. Any other warning is a shell error to look at.
    assert!(
        !warned || !calls.refused.is_empty(),
        "the shell warned without a refused call: {list}"
    );
    calls
}

fn registrations(boosts: &Boosts) -> Vec<(String, String)> {
    boosts
        .iter()
        .filter(|(name, _, _)| name.kind() == BoostKind::Script)
        .filter_map(|(name, _, compiled)| {
            let code = compiled?.code.as_deref().filter(|code| !code.is_empty())?;
            Some((name.to_string(), once_per_document(name, code)))
        })
        .collect()
}

fn stored(boosts: &Boosts) -> Value {
    let mut files = serde_json::Map::new();
    let mut builds = serde_json::Map::new();
    for (name, code, compiled) in boosts.iter() {
        files.insert(name.to_string(), code.into());
        if let Some(compiled) = compiled {
            builds.insert(name.to_string(), serde_json::to_value(compiled).unwrap());
        }
    }
    json!({ "boosts": files, "boostsCompiled": builds })
}

/// The shell's stored objects, without keys it never reads: names that don't parse, and
/// builds of Boosts that no longer exist.
fn readable_store(items: &Value) -> Value {
    let files: serde_json::Map<_, _> = items["boosts"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(name, _)| BoostName::parse(name).is_some())
        .map(|(name, code)| (name.clone(), code.clone()))
        .collect();
    let builds: serde_json::Map<_, _> = items["boostsCompiled"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(name, _)| files.contains_key(*name))
        .map(|(name, build)| (name.clone(), build.clone()))
        .collect();
    json!({ "boosts": files, "boostsCompiled": builds })
}

fn kind(value: &Value) -> Option<BoostKind> {
    serde_json::from_value(value.clone()).ok()
}

/// An intended difference: the rewrite applies Boosts to http and https pages only.
///
/// The shell didn't. `boostMatches` takes any scheme with a host, and the shell was a component
/// extension (patch 0021), so Chromium let it style and script `chrome://` and extension pages.
/// It had no `file:` access, but `chrome.tabs.reload` still reloaded `file:` tabs. On non-web
/// pages these tests check the fixture against this model of the shell, and the crate against
/// its own rule, so a change on either side fails here.
mod web_only_deviation {
    use flecha_boosts::compile::once_per_document;
    use flecha_boosts::{BoostKind, Boosts, Frame, boost_matches};

    /// Chromium refused the shell's style and script calls on `file:` pages.
    pub fn shell_could_script(url: &str) -> bool {
        !url.starts_with("file:")
    }

    /// The styles the shell inserted on a page.
    pub fn shell_styles(boosts: &Boosts, frame: Frame, url: &str) -> Vec<String> {
        if frame != Frame::Main || !shell_could_script(url) {
            return Vec::new();
        }
        boosts
            .iter()
            .filter(|(name, _, _)| {
                name.kind() == BoostKind::Styles && boost_matches(url, name.host())
            })
            .map(|(_, code, _)| code.to_owned())
            .collect()
    }

    /// What the shell's execute path ran or reported on a page.
    pub fn shell_scripts(boosts: &Boosts, frame: Frame, url: &str) -> Vec<(String, String)> {
        if frame != Frame::Main || !shell_could_script(url) {
            return Vec::new();
        }
        boosts
            .iter()
            .filter(|(name, _, _)| {
                name.kind() == BoostKind::Script && boost_matches(url, name.host())
            })
            .filter_map(|(name, _, compiled)| {
                let compiled = compiled?;
                match (compiled.code.as_deref(), compiled.error.as_deref()) {
                    (Some(code), _) if !code.is_empty() => {
                        Some(("run".into(), once_per_document(name, code)))
                    }
                    (_, Some(error)) if !error.is_empty() => {
                        Some(("error".into(), format!("[Flecha boost {name}] {error}")))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// Which shell effects on non-web pages the fixture showed, so the deviation stays covered.
    #[derive(Debug, Default, PartialEq)]
    pub struct Seen {
        pub styled: bool,
        pub ran: bool,
        pub reported: bool,
        pub restyled: bool,
        pub reloaded: bool,
        pub reloaded_file: bool,
        pub refused_file: bool,
    }
}

use web_only_deviation::{Seen, shell_could_script, shell_scripts, shell_styles};

fn effect_matches(
    effect: &Effect,
    tabs: &[(u64, String)],
    boosts: &Boosts,
    calls: &Calls,
    seen: &mut Seen,
) {
    for (tab, url) in tabs {
        let affected = effect.affects(url);
        let shell_reloaded = calls.reloads.contains(tab);
        let shell_restyled = calls.styles.get(tab);
        if !is_web(url) {
            assert!(!affected, "the rewrite leaves non-web tab {url} alone");
            let matched = match effect {
                Effect::Restyle { host } | Effect::Reload { host } => boost_matches(url, host),
                Effect::Unchanged | Effect::Saved => false,
            };
            let reloaded = matches!(effect, Effect::Reload { .. }) && matched;
            assert_eq!(reloaded, shell_reloaded, "the shell reloaded {url}");
            let restyle = matches!(effect, Effect::Restyle { .. }) && matched;
            let restyled = (restyle && shell_could_script(url))
                .then(|| shell_styles(boosts, Frame::Main, url));
            assert_eq!(
                restyled.as_ref(),
                shell_restyled,
                "the shell restyled {url}"
            );
            assert_eq!(
                restyle && !shell_could_script(url),
                calls.refused.contains(tab),
                "Chromium refused the shell on {url}"
            );
            seen.restyled |= restyled.is_some();
            seen.reloaded |= reloaded;
            seen.reloaded_file |= reloaded && !shell_could_script(url);
            seen.refused_file |= calls.refused.contains(tab);
            continue;
        }
        assert_eq!(
            matches!(effect, Effect::Reload { .. }) && affected,
            shell_reloaded,
            "reload of {url} after {effect:?}"
        );
        let restyled = (matches!(effect, Effect::Restyle { .. }) && affected).then(|| {
            boosts
                .styles_for(Frame::Main, url)
                .into_iter()
                .map(|style| style.css)
                .collect::<Vec<_>>()
        });
        assert_eq!(
            restyled.as_ref(),
            shell_restyled,
            "styles of {url} after {effect:?}"
        );
    }
}

#[test]
fn boosts_store_and_apply_as_in_the_shell() {
    assert_eq!(StyleInjection::RUN_AT, RunAt::DocumentStart);
    let mut seen = Seen::default();
    for scenario in rows("scenario") {
        let name = str_of(&scenario["name"]);
        let stored_before = &scenario["stored"];
        let import = Boosts::from_shell(
            stored_before.get("boosts"),
            stored_before.get("boostsCompiled"),
        );
        let unreadable: &[&str] = if name.starts_with("malformed builds") {
            &[SHELL_BUILDS_KEY]
        } else if name.starts_with("malformed files") {
            &[SHELL_FILES_KEY]
        } else {
            &[]
        };
        assert_eq!(import.unreadable, unreadable, "{name}: unreadable keys");
        let mut boosts = import.boosts;
        let tabs: Vec<(u64, String)> = scenario["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tab| (tab["id"].as_u64().unwrap(), str_of(&tab["url"]).to_owned()))
            .collect();
        let startup = calls(&scenario["startup"]);
        assert_eq!(
            startup.registered.unwrap_or_default(),
            registrations(&boosts),
            "{name}: startup"
        );

        for step in scenario["steps"].as_array().unwrap() {
            let op = &step["do"];
            let context = format!("{name}: {op}");
            let calls = calls(&step["calls"]);
            let expected = &step["result"];
            let effect = match str_of(&op["op"]) {
                "create" | "fileFor" => {
                    let created = kind(&op["kind"]).and_then(|kind| {
                        if op["op"] == "create" {
                            boosts.create(str_of(&op["host"]), kind)
                        } else {
                            boosts.file_for(op["url"].as_str(), kind)
                        }
                    });
                    let (result, effect) = created
                        .map_or((Value::Null, Effect::Unchanged), |(name, effect)| {
                            (name.to_string().into(), effect)
                        });
                    assert_eq!(&result, expected, "{context}");
                    effect
                }
                "save" => {
                    let build: Option<CompiledBoost> = op
                        .get("build")
                        .map(|build| serde_json::from_value(build.clone()).unwrap());
                    let reload = op["reload"].as_bool().unwrap_or(true);
                    let saved =
                        boosts.save(str_of(&op["name"]), str_of(&op["code"]), build, reload);
                    let (result, effect) = saved
                        .map_or((Value::Null, Effect::Unchanged), |(file, effect)| {
                            (serde_json::to_value(file).unwrap(), effect)
                        });
                    assert_eq!(&result, expected, "{context}");
                    effect
                }
                "remove" => boosts.remove(str_of(&op["name"])),
                "list" => {
                    assert_eq!(
                        &serde_json::to_value(boosts.list()).unwrap(),
                        expected,
                        "{context}"
                    );
                    Effect::Unchanged
                }
                "get" => {
                    let file = boosts.get(str_of(&op["name"]));
                    assert_eq!(&serde_json::to_value(file).unwrap(), expected, "{context}");
                    Effect::Unchanged
                }
                "boostedHosts" => {
                    assert_eq!(&json!(boosts.boosted_hosts()), expected, "{context}");
                    Effect::Unchanged
                }
                "commit" => {
                    let frame = if op["frameId"] == 0 {
                        Frame::Main
                    } else {
                        Frame::Sub
                    };
                    let url = str_of(&op["url"]);
                    let css: Vec<String> = boosts
                        .styles_for(frame, url)
                        .into_iter()
                        .map(|s| s.css)
                        .collect();
                    let tab = op["tabId"].as_u64().unwrap();
                    let shell = calls.styles.get(&tab).cloned().unwrap_or_default();
                    if is_web(url) {
                        assert_eq!(css, shell, "{context}");
                    } else {
                        assert!(
                            css.is_empty(),
                            "{context}: the rewrite styles web pages only"
                        );
                        assert_eq!(shell, shell_styles(&boosts, frame, url), "{context}: shell");
                        seen.styled |= !shell.is_empty();
                        seen.refused_file |= calls.refused.contains(&tab);
                    }
                    continue;
                }
                "domReady" => {
                    let frame = if op["frameId"] == 0 {
                        Frame::Main
                    } else {
                        Frame::Sub
                    };
                    let url = str_of(&op["url"]);
                    // The fixture records the shell's execute path. Chromium's registered user
                    // scripts may add more, checked in registered_user_scripts_follow_chromium.
                    let (executed, registered): (Vec<_>, Vec<_>) = boosts
                        .scripts_for(frame, url)
                        .into_iter()
                        .partition(|script| applies(url, script.name().host()));
                    for script in &registered {
                        assert!(matches!(script, ScriptInjection::Run { .. }), "{context}");
                        assert!(user_script_matches(url, script.name().host()), "{context}");
                    }
                    let scripts: Vec<(String, String)> = executed
                        .into_iter()
                        .map(|script| match script {
                            ScriptInjection::Run { source, .. } => ("run".into(), source),
                            ScriptInjection::ReportError { message, .. } => {
                                ("error".into(), message)
                            }
                        })
                        .collect();
                    if is_web(url) {
                        assert_eq!(scripts, calls.scripts, "{context}");
                    } else {
                        assert!(
                            scripts.is_empty() && registered.is_empty(),
                            "{context}: the rewrite scripts web pages only"
                        );
                        let shell = shell_scripts(&boosts, frame, url);
                        assert_eq!(calls.scripts, shell, "{context}: shell");
                        seen.ran |= shell.iter().any(|(what, _)| what == "run");
                        seen.reported |= shell.iter().any(|(what, _)| what == "error");
                        seen.refused_file |= !calls.refused.is_empty();
                    }
                    continue;
                }
                op => panic!("unknown op {op}"),
            };
            assert_eq!(
                effect.changed(),
                calls.stored.is_some(),
                "{context}: changed"
            );
            if let Some(items) = &calls.stored {
                let (ours, shell) = (stored(&boosts), readable_store(items));
                assert_eq!(ours, shell, "{context}: stored");
                // Map equality ignores order, and Boosts apply in the stored order.
                let names = |items: &Value| {
                    items["boosts"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                };
                assert_eq!(names(&ours), names(&shell), "{context}: stored order");
            }
            if let Some(registered) = &calls.registered {
                assert_eq!(registered, &registrations(&boosts), "{context}: registered");
            }
            effect_matches(&effect, &tabs, &boosts, &calls, &mut seen);
        }
    }
    let every = Seen {
        styled: true,
        ran: true,
        reported: true,
        restyled: true,
        reloaded: true,
        reloaded_file: true,
        refused_file: true,
    };
    assert_eq!(
        seen, every,
        "the fixture covers each shell effect on non-web pages"
    );
}

fn shell_transform(row: &Value) -> impl FnOnce(&str, &str) -> Result<String, String> + '_ {
    move |name, source| {
        assert_eq!(name, str_of(&row["name"]));
        assert_eq!(source, str_of(&row["source"]));
        match row["transform"].get("code") {
            Some(code) => Ok(str_of(code).to_owned()),
            None => Err(str_of(&row["transform"]["error"]).to_owned()),
        }
    }
}

/// Hostile Boost text, compiled by the shell's sucrase and wrapped here.
///
/// The wrapped text must equal the shell's byte for byte; `runs` records what that text did when
/// the generator ran it twice in one Node `vm` context, as both of the shell's injection paths can.
#[test]
fn hostile_boosts_compile_and_run_as_in_the_shell() {
    for row in rows("compile") {
        let case = str_of(&row["case"]);
        let name = BoostName::parse(str_of(&row["name"])).unwrap();
        let compiled = compile_with(&name, str_of(&row["source"]), shell_transform(&row));
        assert_eq!(
            serde_json::to_value(&compiled).unwrap(),
            row["compiled"],
            "{case}"
        );

        let Some(code) = compiled.code.as_deref() else {
            assert!(row["injected"].is_null(), "{case}");
            continue;
        };
        let mut boosts = Boosts::new();
        boosts
            .save(name.as_str(), "", Some(compiled.clone()), true)
            .unwrap();
        let injected = boosts.scripts_for(Frame::Main, &format!("https://{}/", name.host()));
        let [ScriptInjection::Run { source, .. }] = injected.as_slice() else {
            panic!("{case}: {injected:?}");
        };
        assert_eq!(source, &once_per_document(&name, code));
        assert_eq!(source, str_of(&row["injected"]), "{case}");

        let runs = &row["runs"];
        let evil = runs["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e[0] == "evil")
            .count();
        let marks = runs["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e[0] == "mark")
            .count();
        match case {
            // Annex B: a classic script reads `<!--` as a comment, which sucrase's module parse
            // doesn't. The text after it closes the wrapper and runs outside it.
            "html-open-comment" => {
                assert_eq!((evil, marks), (1, 1), "{case}: escapes the function")
            }
            "html-open-comment-guard" => {
                assert_eq!(
                    (evil, marks),
                    (2, 2),
                    "{case}: escapes the once-per-document guard"
                )
            }
            "await-top" => assert_eq!(runs["outcomes"], json!(["SyntaxError", "SyntaxError"])),
            _ => {
                assert_eq!(evil, 0, "{case}: hostile text ran");
                assert!(marks <= 1, "{case}: ran twice in one document");
                assert_eq!(runs["outcomes"], json!(["ran", "ran"]), "{case}");
            }
        }
    }
}

#[test]
fn a_large_boost_compiles_as_in_the_shell() {
    let row = rows("compile-large").remove(0);
    let sha = |text: &str| format!("{:x}", Sha256::digest(text.as_bytes()));
    let source = str_of(&row["line"]).repeat(row["repeat"].as_u64().unwrap() as usize);
    assert_eq!(sha(&source), row["sourceSha256"]);
    // For this input, sucrase only prepends "use strict".
    let transformed = format!("\"use strict\";{source}");
    assert_eq!(sha(&transformed), row["transformSha256"]);
    let name = BoostName::parse(str_of(&row["name"])).unwrap();
    let compiled = compile_with(&name, &source, |_, _| Ok(transformed));
    let code = compiled.code.unwrap();
    assert_eq!(sha(&code), row["compiledSha256"]);
    assert_eq!(sha(&once_per_document(&name, &code)), row["injectedSha256"]);
}

/// Chromium's URLPattern (`extensions/common/url_pattern.cc`) canonicalizes the pattern host and
/// trims trailing dots, so the shell's registered scripts ran on pages `boostMatches` refuses.
#[test]
fn registered_user_scripts_follow_chromium() {
    let scenario = rows("scenario")
        .into_iter()
        .find(|row| row["name"] == "registered user scripts")
        .unwrap();
    let stored = &scenario["stored"];
    let boosts = Boosts::from_shell(stored.get("boosts"), stored.get("boostsCompiled")).boosts;
    let expected = [
        ("https://github.com./", vec!["github.com.ts", "all.ts"]),
        (
            "https://gist.github.com../",
            vec!["github.com.ts", "all.ts"],
        ),
        ("https://shout.example/", vec!["Shout.EXAMPLE.ts", "all.ts"]),
        (
            "https://a.shout.example:8443/",
            vec!["Shout.EXAMPLE.ts", "all.ts"],
        ),
        ("http://Shout.Example./", vec!["Shout.EXAMPLE.ts", "all.ts"]),
        ("https://example.org/", vec!["all.ts"]),
        ("https://notgithub.com/", vec!["all.ts"]),
    ];
    let steps = scenario["steps"].as_array().unwrap();
    assert_eq!(steps.len(), expected.len());
    for (step, (url, names)) in steps.iter().zip(expected) {
        assert_eq!(step["do"]["url"], url);
        let ran: Vec<String> = boosts
            .scripts_for(Frame::Main, url)
            .iter()
            .map(|script| script.name().to_string())
            .collect();
        assert_eq!(ran, names, "{url}");
    }
    for (url, host) in [
        ("http://127.0.0.1/", "0.1"),
        ("https://GITHUB.com/", "GitHub.COM"),
        ("https://xn--bcher-kva.example/", "xn--bcher-kva.example"),
    ] {
        let subdomain_of_ip = url.contains("127.0.0.1");
        assert_eq!(
            user_script_matches(url, host),
            !subdomain_of_ip,
            "{url} {host}"
        );
    }
}
