//! JSON, XML and GraphQL colouring for bodies, as egui layout jobs. Results are
//! cached by content so large responses aren't re-tokenised every frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

use crate::theme::Palette;

const LIMIT: usize = 300_000;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Json,
    Xml,
    GraphQl,
    Plain,
}

impl Lang {
    /// Picks a language from a content type, falling back to sniffing the text.
    pub fn detect(content_type: &str, text: &str) -> Lang {
        let ct = content_type.to_ascii_lowercase();
        let t = text.trim_start();
        if ct.contains("json") || t.starts_with('{') || t.starts_with('[') {
            Lang::Json
        } else if ct.contains("xml") || ct.contains("soap") || ct.contains("html") || t.starts_with('<') {
            Lang::Xml
        } else {
            Lang::Plain
        }
    }
}

#[derive(Default)]
pub struct Highlighter {
    cache: RefCell<HashMap<u64, LayoutJob>>,
}

impl Highlighter {
    pub fn job(&self, text: &str, lang: Lang, p: &Palette, font: &FontId, wrap_width: f32) -> LayoutJob {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        lang.hash(&mut h);
        p.text.hash(&mut h);
        font.size.to_bits().hash(&mut h);
        let key = h.finish();
        let mut job = {
            let mut cache = self.cache.borrow_mut();
            if let Some(j) = cache.get(&key) {
                j.clone()
            } else {
                let j = build(text, lang, p, font);
                if cache.len() > 48 {
                    cache.clear();
                }
                cache.insert(key, j.clone());
                j
            }
        };
        job.wrap.max_width = wrap_width;
        job
    }
}

fn fmt(font: &FontId, color: Color32) -> TextFormat {
    TextFormat { font_id: font.clone(), color, ..Default::default() }
}

fn build(text: &str, lang: Lang, p: &Palette, font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    if text.len() > LIMIT {
        job.append(text, 0.0, fmt(font, p.text));
        return job;
    }
    match lang {
        Lang::Json => json(&mut job, text, p, font),
        Lang::Xml => xml(&mut job, text, p, font),
        Lang::GraphQl => graphql(&mut job, text, p, font),
        Lang::Plain => job.append(text, 0.0, fmt(font, p.text)),
    }
    job
}

fn json(job: &mut LayoutJob, text: &str, p: &Palette, font: &FontId) {
    let b = text.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;
    let flush = |job: &mut LayoutJob, from: usize, to: usize| {
        if to > from {
            job.append(&text[from..to], 0.0, fmt(font, p.text2));
        }
    };
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            flush(job, plain_start, i);
            let start = i;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            let mut j = i;
            while j < b.len() && (b[j] as char).is_whitespace() {
                j += 1;
            }
            let color = if j < b.len() && b[j] == b':' { p.j_key } else { p.j_str };
            job.append(&text[start..i], 0.0, fmt(font, color));
            plain_start = i;
        } else if c == b'-' || c.is_ascii_digit() {
            flush(job, plain_start, i);
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || matches!(b[i], b'-' | b'+' | b'.' | b'e' | b'E')) {
                i += 1;
            }
            job.append(&text[start..i], 0.0, fmt(font, p.j_num));
            plain_start = i;
        } else if let Some(w) = ["true", "false", "null"].into_iter().find(|w| text[i..].starts_with(w)) {
            flush(job, plain_start, i);
            job.append(w, 0.0, fmt(font, p.j_lit));
            i += w.len();
            plain_start = i;
        } else {
            // Step over a whole character so slicing stays on boundaries.
            i += match c {
                0xF0.. => 4,
                0xE0.. => 3,
                0xC0.. => 2,
                _ => 1,
            };
        }
    }
    flush(job, plain_start, b.len());
}

fn graphql(job: &mut LayoutJob, text: &str, p: &Palette, font: &FontId) {
    const KEYWORDS: [&str; 10] = ["query", "mutation", "subscription", "fragment", "on", "true", "false", "null", "schema", "type"];
    let b = text.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;
    let flush = |job: &mut LayoutJob, from: usize, to: usize| {
        if to > from {
            job.append(&text[from..to], 0.0, fmt(font, p.text));
        }
    };
    let word_end = |mut j: usize| {
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
            j += 1;
        }
        j
    };
    while i < b.len() {
        let c = b[i];
        let (end, format) = if c == b'#' {
            let end = text[i..].find('\n').map(|n| i + n).unwrap_or(b.len());
            (end, TextFormat { italics: true, ..fmt(font, p.faint) })
        } else if c == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' && b[j] != b'\n' {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            ((j + 1).min(b.len()), fmt(font, p.j_str))
        } else if c == b'$' {
            (word_end(i + 1), fmt(font, p.x_attr))
        } else if c == b'@' {
            (word_end(i + 1), fmt(font, p.j_lit))
        } else if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_digit() || matches!(b[j], b'.' | b'e' | b'E' | b'+' | b'-')) {
                j += 1;
            }
            (j, fmt(font, p.j_num))
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let end = word_end(i);
            let w = &text[i..end];
            let color = if KEYWORDS.contains(&w) {
                p.j_lit
            } else if w.starts_with(|ch: char| ch.is_ascii_uppercase()) {
                p.x_tag
            } else {
                p.j_key
            };
            (end, fmt(font, color))
        } else {
            i += text[i..].chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        flush(job, plain_start, i);
        job.append(&text[i..end], 0.0, format);
        i = end;
        plain_start = end;
    }
    flush(job, plain_start, b.len());
}

fn xml(job: &mut LayoutJob, text: &str, p: &Palette, font: &FontId) {
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        if lt > 0 {
            job.append(&rest[..lt], 0.0, fmt(font, p.text));
        }
        let after = &rest[lt..];
        let end = if after.starts_with("<!--") { after.find("-->").map(|e| e + 3) } else { after.find('>').map(|e| e + 1) };
        let Some(end) = end else {
            job.append(after, 0.0, fmt(font, p.text));
            return;
        };
        let tag = &after[..end];
        if tag.starts_with("<!--") {
            job.append(tag, 0.0, TextFormat { italics: true, ..fmt(font, p.faint) });
        } else {
            let name_end = tag[1..].find(|c: char| c.is_whitespace() || c == '>' || (c == '/' && tag.len() > 2)).map(|n| n + 1).unwrap_or(tag.len());
            let name_end = if tag[1..].starts_with('/') || tag[1..].starts_with('?') {
                tag[2..].find(|c: char| c.is_whitespace() || c == '>' || c == '?').map(|n| n + 2).unwrap_or(tag.len())
            } else {
                name_end
            };
            job.append(&tag[..name_end], 0.0, fmt(font, p.x_tag));
            let mut attrs = &tag[name_end..];
            while let Some(eq) = attrs.find('=') {
                let q = attrs[eq + 1..].chars().next().unwrap_or(' ');
                if q != '"' && q != '\'' {
                    break;
                }
                let Some(close) = attrs[eq + 2..].find(q) else { break };
                let val_end = eq + 2 + close + 1;
                job.append(&attrs[..eq], 0.0, fmt(font, p.x_attr));
                job.append("=", 0.0, fmt(font, p.text2));
                job.append(&attrs[eq + 1..val_end], 0.0, fmt(font, p.x_val));
                attrs = &attrs[val_end..];
            }
            if !attrs.is_empty() {
                job.append(attrs, 0.0, fmt(font, p.x_tag));
            }
        }
        rest = &after[end..];
    }
    if !rest.is_empty() {
        job.append(rest, 0.0, fmt(font, p.text));
    }
}
