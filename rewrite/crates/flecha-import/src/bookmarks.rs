use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use flecha_core::input_text::{JS_SPACE_CHARS, is_js_whitespace};
use regex::{Captures, Regex};

use crate::input::{read_selected_text, web_scheme};
use crate::{Bookmark, ImportPlan, ReadError, SavedTab};

// Shipping browserImport.ts shares both limits across a browser's profiles.
const MAX_BOOKMARKS: usize = 2000;
const MAX_DEPTH: usize = 8;

#[derive(Default)]
pub(crate) struct BookmarkTree {
    root: Vec<Bookmark>,
    folders: Vec<(String, Vec<Bookmark>)>,
}

impl BookmarkTree {
    pub(crate) fn push(&mut self, node: Bookmark) {
        match self.folders.last_mut() {
            Some((_, children)) => children.push(node),
            None => self.root.push(node),
        }
    }

    pub(crate) fn open(&mut self, name: String) {
        self.folders.push((name, Vec::new()));
    }

    pub(crate) fn close(&mut self, keep_empty: bool) {
        let (name, children) = self.folders.pop().expect("an open bookmark folder");
        if keep_empty || !children.is_empty() {
            self.push(Bookmark::Folder { name, children });
        }
    }

    pub(crate) fn finish(mut self) -> Vec<Bookmark> {
        while !self.folders.is_empty() {
            self.close(true);
        }
        self.root
    }
}

#[derive(Default)]
pub(crate) struct Tidy {
    seen: HashSet<String>,
    skipped: usize,
}

impl Tidy {
    pub(crate) fn run(&mut self, nodes: &[Bookmark]) -> Vec<Bookmark> {
        let mut tree = BookmarkTree::default();
        let mut frames = vec![(nodes.iter(), false)];
        while let Some((nodes, opened_folder)) = frames.last_mut() {
            match nodes.next() {
                Some(Bookmark::Tab(tab))
                    if web_scheme(&tab.url) && !self.seen.contains(&tab.url) =>
                {
                    if self.seen.len() >= MAX_BOOKMARKS {
                        self.skipped += 1;
                    } else {
                        self.seen.insert(tab.url.clone());
                        tree.push(Bookmark::Tab(tab.clone()));
                    }
                }
                Some(Bookmark::Folder { name, children }) => {
                    let open = tree.folders.len() < MAX_DEPTH;
                    if open {
                        tree.open(name.clone());
                    }
                    frames.push((children.iter(), open));
                }
                Some(Bookmark::Tab(_)) => {}
                None => {
                    if *opened_folder {
                        tree.close(false);
                    }
                    frames.pop();
                }
            }
        }
        tree.finish()
    }

    pub(crate) fn finish(self, plan: &mut ImportPlan, bookmarks: Vec<Bookmark>) {
        if self.skipped > 0 {
            plan.warnings.push(format!(
                "Only the first {MAX_BOOKMARKS} bookmarks were imported; {} more were left out.",
                self.skipped
            ));
        }
        if !bookmarks.is_empty() {
            plan.bookmark_count = self.seen.len();
            plan.bookmarks = Some(bookmarks);
        }
    }
}

pub fn read_bookmarks_html_file(file: &Path) -> Result<ImportPlan, ReadError> {
    Ok(read_bookmarks_html(&read_selected_text(file)?))
}

pub fn read_bookmarks_html(html: &str) -> ImportPlan {
    let mut plan = ImportPlan::new("Imported Bookmarks");
    let mut tidy = Tidy::default();
    let bookmarks = tidy.run(&parse_html(html));
    tidy.finish(&mut plan, bookmarks);
    if plan.bookmarks.is_none() {
        plan.warnings.push("No bookmarks were found in the file. Choose the HTML file a browser saves when it exports its bookmarks.".into());
    }
    plan
}

fn parse_html(html: &str) -> Vec<Bookmark> {
    static TAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)<(/?)(dl|h3|a)(?-u:\b)([^>]*)>").expect("static markup pattern")
    });
    static ANCHOR_END: LazyLock<Regex> = LazyLock::new(|| close_tag("a"));
    static HEADING_END: LazyLock<Regex> = LazyLock::new(|| close_tag("h3"));
    static TOOLBAR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(r#"(?i-u:\bPERSONAL_TOOLBAR_FOLDER)[{JS_SPACE_CHARS}]*=[{JS_SPACE_CHARS}]*["']?(?i-u:true)"#))
            .expect("static toolbar attribute")
    });
    static HREF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(r#"(?i)(?-u:\b)HREF[{JS_SPACE_CHARS}]*=[{JS_SPACE_CHARS}]*(?:"([^"]*)"|'([^']*)'|([^{JS_SPACE_CHARS}>]+))"#))
            .expect("static address attribute")
    });
    static TAGS_IN_TITLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<[^>]*>").expect("static title markup pattern"));

    let mut tree = BookmarkTree::default();
    let mut lists = Vec::new();
    let mut heading = None;
    let mut at = 0;
    while let Some(tag) = TAG.captures_at(html, at) {
        at = tag.get(0).expect("full tag").end();
        let closing = !tag[1].is_empty();
        let kind = &tag[2];
        if kind.eq_ignore_ascii_case("dl") {
            if closing {
                if lists.pop() == Some(true) {
                    tree.close(true);
                }
            } else {
                let open = if let Some((name, toolbar)) = heading.take() {
                    if toolbar && tree.folders.is_empty() {
                        false
                    } else {
                        tree.open(name);
                        true
                    }
                } else {
                    false
                };
                lists.push(open);
            }
        } else if !closing {
            let is_heading = kind.eq_ignore_ascii_case("h3");
            let end_tag = if is_heading {
                &*HEADING_END
            } else {
                &*ANCHOR_END
            };
            let end = end_tag.find_at(html, at);
            let title =
                TAGS_IN_TITLE.replace_all(&html[at..end.map_or(html.len(), |m| m.start())], "");
            let title = decode_html(&title)
                .split(is_js_whitespace)
                .filter(|word| !word.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            at = end.map_or(html.len(), |m| m.end());
            if is_heading {
                let name = if title.is_empty() {
                    "Untitled folder".into()
                } else {
                    title
                };
                heading = Some((name, TOOLBAR.is_match(&tag[3])));
            } else if let Some(url) = HREF
                .captures(&tag[3])
                .and_then(|m| m.iter().skip(1).flatten().next())
            {
                let url = decode_html(url.as_str()).into_owned();
                if !url.is_empty() {
                    let title = if title.is_empty() { url.clone() } else { title };
                    tree.push(Bookmark::Tab(SavedTab { url, title }));
                }
            }
        }
    }
    tree.finish()
}

fn close_tag(tag: &str) -> Regex {
    Regex::new(&format!(r"(?i)</{tag}[{JS_SPACE_CHARS}]*>")).expect("static closing tag")
}

fn decode_html(text: &str) -> std::borrow::Cow<'_, str> {
    static ENTITY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)&(#x[0-9a-f]+|#[0-9]+|[a-z]+);").expect("static entity pattern")
    });
    let mut matches = ENTITY.captures_iter(text).peekable();
    if matches.peek().is_none() {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut decoded = String::new();
    let mut at = 0;
    while let Some(entity) = matches.next() {
        let found = entity.get(0).expect("full entity");
        decoded.push_str(&text[at..found.start()]);
        at = found.end();
        let key = entity[1].to_ascii_lowercase();
        let code = numeric_entity(&key);
        if let Some(high @ 0xd800..=0xdbff) = code
            && let Some(low @ 0xdc00..=0xdfff) = matches.peek().and_then(|next: &Captures<'_>| {
                (next.get(0)?.start() == at)
                    .then(|| numeric_entity(&next[1].to_ascii_lowercase()))
                    .flatten()
            })
        {
            let scalar = 0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00);
            decoded.push(char::from_u32(scalar).expect("a UTF-16 surrogate pair"));
            at = matches
                .next()
                .and_then(|next| next.get(0))
                .expect("peeked entity")
                .end();
            continue;
        }
        match (key.as_str(), code) {
            ("lt", _) => decoded.push('<'),
            ("gt", _) => decoded.push('>'),
            ("amp", _) => decoded.push('&'),
            ("quot", _) => decoded.push('"'),
            ("apos", _) => decoded.push('\''),
            ("nbsp", _) => decoded.push('\u{a0}'),
            (_, Some(0xd800..=0xdfff)) => decoded.push('\u{fffd}'),
            (_, code) => match code.and_then(char::from_u32) {
                Some(ch) => decoded.push(ch),
                None => decoded.push_str(found.as_str()),
            },
        }
    }
    decoded.push_str(&text[at..]);
    std::borrow::Cow::Owned(decoded)
}

fn numeric_entity(entity: &str) -> Option<u32> {
    if let Some(hex) = entity.strip_prefix("#x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        entity.strip_prefix('#')?.parse().ok()
    }
}
