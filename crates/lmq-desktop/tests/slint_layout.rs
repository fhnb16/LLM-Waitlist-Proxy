//! Страж от бага высоты текста с переносом в Slint 1.18 (`examples/wrap_probe.rs` его
//! воспроизводит). Внутри `if`/`for` высота по ширине теряется, и текст с переносом получает
//! место под одну строку: строки наползают друг на друга, низ страницы обрезается. Это
//! случается, когда в содержимом повторителя:
//!
//! - `Text { wrap: … }` стоит не корнем повторителя, а внутри раскладки или элемента;
//! - компонент, в котором есть текст с переносом, получает на месте использования дочерние
//!   элементы (`SettingRow { Button { … } }`) или привязку к свойству, которое он сам не
//!   объявлял (`Paragraph { padding: 16px; }` — свойство раскладки).
//!
//! Верно считается компонент без дочерних элементов с привязками к своим свойствам
//! (`if x: Paragraph { pad-x: 16px; text: … }`) и сам `Text` корнем повторителя. Тест
//! разбирает ui/*.slint и требует только такие формы.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str,
    Punct(&'static str),
}

fn tokenize(src: &str) -> Vec<(Tok, usize)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_ascii_whitespace() => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
                out.push((Tok::Str, line));
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 => {
                let s = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'-' || b[i] >= 0x80) {
                    i += 1;
                }
                out.push((Tok::Ident(src[s..i].to_string()), line));
            }
            _ => {
                let p: &'static str = match (c, b.get(i + 1), b.get(i + 2)) {
                    (b'<', Some(b'='), Some(b'>')) => "<=>",
                    (b':', Some(b'='), _) => ":=",
                    (b'=', Some(b'>'), _) => "=>",
                    (b':', _, _) => ":",
                    (b'{', _, _) => "{",
                    (b'}', _, _) => "}",
                    (b'(', _, _) => "(",
                    (b')', _, _) => ")",
                    (b'[', _, _) => "[",
                    (b']', _, _) => "]",
                    (b'<', _, _) => "<",
                    (b'>', _, _) => ">",
                    (b';', _, _) => ";",
                    (b'@', _, _) => "@",
                    _ => "?",
                };
                i += p.len();
                out.push((Tok::Punct(p), line));
            }
        }
    }
    out
}

#[derive(Debug, Default)]
struct Node {
    ty: String,
    line: usize,
    wrap: bool,
    repeater: bool,
    children: Vec<Node>,
    /// Свойства, которым элемент задаёт значение (`text: …`, `current <=> …`).
    bindings: Vec<String>,
    /// Свойства, объявленные в теле (`in property <string> text`) — у корня компонента.
    declared: Vec<String>,
}

/// Что за оператор разобран в теле элемента.
enum Stmt {
    Other,
    Wrap,
    Binding(String),
    Declared(String),
}

struct Parser {
    t: Vec<(Tok, usize)>,
    i: usize,
}

impl Parser {
    fn peek(&self, k: usize) -> Option<&Tok> {
        self.t.get(self.i + k).map(|x| &x.0)
    }
    fn is(&self, k: usize, p: &str) -> bool {
        matches!(self.peek(k), Some(Tok::Punct(x)) if *x == p)
    }
    fn ident(&self, k: usize) -> Option<&str> {
        match self.peek(k) {
            Some(Tok::Ident(s)) => Some(s),
            _ => None,
        }
    }
    fn line(&self) -> usize {
        self.t.get(self.i).map(|x| x.1).unwrap_or(0)
    }

    /// Пропускает сбалансированную группу, начиная с открывающей скобки.
    fn skip_group(&mut self) {
        let mut depth = 0i32;
        while let Some(tok) = self.peek(0).cloned() {
            self.i += 1;
            if let Tok::Punct(p) = tok {
                match p {
                    "{" | "(" | "[" => depth += 1,
                    "}" | ")" | "]" => depth -= 1,
                    _ => {}
                }
            }
            if depth <= 0 {
                return;
            }
        }
    }

    /// Пропускает оператор (свойство, обработчик, анимацию) до `;` или до конца блока `{…}`
    /// и говорит, что это было.
    fn skip_statement(&mut self) -> Stmt {
        let first = self.ident(0).map(str::to_string);
        let stmt = match first.as_deref() {
            // Любой перенос, кроме явного `no-wrap` (в том числе выражение `a ? char-wrap : word-wrap`).
            Some("wrap") if self.is(1, ":") && self.ident(2) != Some("no-wrap") => Stmt::Wrap,
            Some("in" | "out" | "in-out" | "property") => {
                // `[in|out|in-out] property <тип> имя …`
                let mut k = 0;
                while self.ident(k).is_some_and(|s| s != "property") && k < 2 {
                    k += 1;
                }
                if self.ident(k) == Some("property") && self.is(k + 1, "<") {
                    let mut j = k + 2;
                    let mut depth = 1;
                    while depth > 0 && self.peek(j).is_some() {
                        if self.is(j, "<") {
                            depth += 1;
                        } else if self.is(j, ">") {
                            depth -= 1;
                        }
                        j += 1;
                    }
                    self.ident(j).map(|s| Stmt::Declared(s.to_string())).unwrap_or(Stmt::Other)
                } else {
                    Stmt::Other
                }
            }
            Some(name) if self.is(1, ":") || self.is(1, "<=>") => Stmt::Binding(name.to_string()),
            _ => Stmt::Other,
        };
        loop {
            match self.peek(0) {
                None => return stmt,
                Some(Tok::Punct(";")) => {
                    self.i += 1;
                    return stmt;
                }
                Some(Tok::Punct("}")) => return stmt,
                Some(Tok::Punct("{")) => {
                    self.skip_group();
                    if self.is(0, ";") {
                        self.i += 1;
                    }
                    return stmt;
                }
                Some(Tok::Punct("(" | "[")) => self.skip_group(),
                _ => self.i += 1,
            }
        }
    }

    /// Элемент: `[id :=] Тип { … }`, курсор — на первом токене.
    fn element(&mut self) -> Node {
        if self.is(1, ":=") {
            self.i += 2;
        }
        let mut n = Node { ty: self.ident(0).unwrap_or("?").to_string(), line: self.line(), ..Default::default() };
        self.i += 1;
        if self.is(0, "{") {
            self.i += 1;
            self.body(&mut n);
        }
        n
    }

    /// Тело элемента до закрывающей `}`.
    fn body(&mut self, n: &mut Node) {
        loop {
            match self.peek(0) {
                None => return,
                Some(Tok::Punct("}")) => {
                    self.i += 1;
                    return;
                }
                Some(Tok::Punct("@")) => self.i += 2,
                Some(Tok::Ident(k)) if k == "if" || k == "for" => {
                    // Условие до `:`, за которым идёт элемент.
                    while self.peek(0).is_some() && !(self.is(0, ":") && self.ident(1).is_some() && (self.is(2, "{") || self.is(2, ":="))) {
                        if matches!(self.peek(0), Some(Tok::Punct("(" | "["))) {
                            self.skip_group();
                        } else {
                            self.i += 1;
                        }
                    }
                    self.i += 1;
                    let mut e = self.element();
                    e.repeater = true;
                    n.children.push(e);
                }
                Some(Tok::Ident(_)) if self.is(1, ":=") || self.is(1, "{") => {
                    let e = self.element();
                    n.children.push(e);
                }
                _ => match self.skip_statement() {
                    Stmt::Wrap => {
                        n.wrap = true;
                        n.bindings.push("wrap".into());
                    }
                    Stmt::Binding(b) => n.bindings.push(b),
                    Stmt::Declared(d) => n.declared.push(d),
                    Stmt::Other => {}
                },
            }
        }
    }
}

/// Компоненты файла: имя → (базовый тип, дерево тела).
fn components(src: &str, out: &mut BTreeMap<String, Node>, file: &str) {
    let mut p = Parser { t: tokenize(src), i: 0 };
    while p.peek(0).is_some() {
        if p.ident(0) == Some("component") {
            let name = p.ident(1).unwrap_or("?").to_string();
            let line = p.line();
            p.i += 2;
            let mut base = String::new();
            if p.ident(0) == Some("inherits") {
                base = p.ident(1).unwrap_or("").to_string();
                p.i += 2;
            }
            let mut root = Node { ty: base, line, ..Default::default() };
            if p.is(0, "{") {
                p.i += 1;
                p.body(&mut root);
            }
            root.ty = format!("{}|{file}", root.ty);
            out.insert(name, root);
        } else if p.is(0, "{") {
            p.skip_group();
        } else {
            p.i += 1;
        }
    }
}

fn has_wrap(n: &Node, wrapping: &BTreeSet<String>) -> bool {
    n.wrap || wrapping.contains(&n.ty) || n.children.iter().any(|c| has_wrap(c, wrapping))
}

/// Что известно о компонентах: в каких есть текст с переносом и какие свойства они объявляют
/// (вместе с базовыми компонентами).
struct Known {
    wrapping: BTreeSet<String>,
    declared: BTreeMap<String, BTreeSet<String>>,
}

impl Known {
    fn from(comps: &BTreeMap<String, Node>) -> Known {
        let base = |root: &Node| root.ty.split('|').next().unwrap_or("").to_string();
        let mut wrapping: BTreeSet<String> = BTreeSet::new();
        loop {
            let before = wrapping.len();
            for (name, root) in comps {
                if has_wrap(root, &wrapping) || wrapping.contains(&base(root)) {
                    wrapping.insert(name.clone());
                }
            }
            if wrapping.len() == before {
                break;
            }
        }
        let mut declared = BTreeMap::new();
        for name in comps.keys() {
            let mut set = BTreeSet::new();
            let mut cur = Some(name.clone());
            while let Some(c) = cur.take() {
                if let Some(root) = comps.get(&c) {
                    set.extend(root.declared.iter().cloned());
                    cur = Some(base(root)).filter(|b| comps.contains_key(b) && *b != c);
                }
            }
            declared.insert(name.clone(), set);
        }
        Known { wrapping, declared }
    }
}

/// Нарушения внутри содержимого повторителя `root`.
fn check_repeater(root: &Node, n: &Node, k: &Known, file: &str, out: &mut Vec<String>) {
    let is_root = std::ptr::eq(root, n);
    if n.wrap && !is_root {
        out.push(format!(
            "{file}:{}: {} с переносом внутри `if`/`for` (строка {}) — вынесите содержимое повторителя в компонент без дочерних элементов",
            n.line, n.ty, root.line
        ));
    }
    if k.wrapping.contains(&n.ty) {
        if !n.children.is_empty() {
            out.push(format!(
                "{file}:{}: {} (в нём текст с переносом) получает дочерние элементы внутри `if`/`for` (строка {}) — сделайте компонент, который сам содержит {} с детьми, и ставьте его без детей",
                n.line, n.ty, root.line, n.ty
            ));
        }
        let own = k.declared.get(&n.ty);
        let foreign: Vec<&str> = n.bindings.iter().map(String::as_str).filter(|b| !own.is_some_and(|d| d.contains(*b))).collect();
        if !foreign.is_empty() {
            out.push(format!(
                "{file}:{}: {} (в нём текст с переносом) внутри `if`/`for` (строка {}) получает чужие свойства {} — задавайте только свойства, объявленные в {} (отступы — его свойствами)",
                n.line,
                n.ty,
                root.line,
                foreign.join(", "),
                n.ty
            ));
        }
    }
    for c in &n.children {
        check_repeater(root, c, k, file, out);
    }
}

fn find_repeaters(n: &Node, k: &Known, file: &str, out: &mut Vec<String>) {
    for c in &n.children {
        if c.repeater {
            check_repeater(c, c, k, file, out);
        }
        find_repeaters(c, k, file, out);
    }
}

#[test]
fn wrapped_text_is_not_measured_inside_repeaters() {
    let dir = Path::new(ROOT).join("ui");
    let mut comps: BTreeMap<String, Node> = BTreeMap::new();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "slint") {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            components(&std::fs::read_to_string(&p).unwrap(), &mut comps, &name);
        }
    }
    assert!(comps.contains_key("SettingRow") && comps.contains_key("Banner"), "разбор .slint не нашёл компоненты");

    // Компоненты с текстом с переносом — с учётом вложенных компонентов и наследования.
    let k = Known::from(&comps);
    assert!(k.wrapping.contains("SettingRow") && k.wrapping.contains("FieldEditor"), "разбор не видит перенос в компонентах: {:?}", k.wrapping);
    assert!(k.declared["Paragraph"].contains("pad-x"), "разбор не видит объявленные свойства: {:?}", k.declared["Paragraph"]);

    let mut problems = Vec::new();
    for root in comps.values() {
        let file = root.ty.split('|').nth(1).unwrap_or("?");
        find_repeaters(root, &k, file, &mut problems);
    }
    problems.sort();
    problems.dedup();
    assert!(problems.is_empty(), "текст с переносом получит высоту одной строки (баг Slint 1.18):\n{}", problems.join("\n"));
}

/// Формы из замера `examples/wrap_probe.rs`: плохие ловятся, хорошие — нет.
#[test]
fn checker_catches_known_bad_shapes() {
    let src = r#"
        component Row inherits HorizontalLayout { in property <string> hint; Text { text: hint; wrap: word-wrap; } @children }
        component Note inherits HorizontalLayout { in-out property <length> pad: 0; Text { wrap: mono ? char-wrap : word-wrap; } }
        component Page inherits VerticalLayout {
            if a: HorizontalLayout { padding: 16px; Text { text: "x"; wrap: word-wrap; } }
            for x in y: Row { hint: x; Button { clicked => { do(); } } }
            if b: Note { padding: 16px; }
            if ok1: Text { text: "ok"; wrap: word-wrap; }
            if ok2: Note { pad: 16px; }
            if ok3: VerticalLayout { Note { } }
            for ok4 in z: Row { hint: ok4; }
            if ok5: Text { text: self.width < 10px ? "a" : "b"; wrap: no-wrap; }
        }
    "#;
    let mut comps = BTreeMap::new();
    components(src, &mut comps, "t.slint");
    let k = Known::from(&comps);
    let mut out = Vec::new();
    find_repeaters(&comps["Page"], &k, "t.slint", &mut out);
    assert_eq!(out.len(), 3, "{out:#?}");
    assert!(out[0].contains("t.slint:5") && out[1].contains("t.slint:6") && out[2].contains("t.slint:7"), "{out:#?}");
    assert!(out[2].contains("padding"), "{out:#?}");
}
