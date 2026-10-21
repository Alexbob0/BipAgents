//! A small regular-expression engine with Python's `re` semantics, for the speech text processing (textproc,
//! numbers_fr, speech_intl, pronunciations, live_brief).
//!
//! Why not `regex`/`fancy_regex`: Python's `\w`, `\d`, `\s` and `\b` are not Rust's (`²` is a word character in
//! Python, combining marks are not…), and compiling Python's classes into `fancy_regex` costs 0.7 to 2.5 MB per
//! pattern: these modules hold some 300 patterns. This engine is a backtracking matcher like Python's `sre`
//! (leftmost-first alternation, greedy and lazy repeats, backreferences, lookahead, fixed-width lookbehind, captures
//! inside lookarounds), working on characters with Python's tables (`pychars`); a compiled pattern is a few hundred
//! bytes.
//!
//! Supported syntax (what the Python modules use): literals and escapes (`\.`, `\n`, `\t`, `\xhh`, `\uhhhh`…),
//! `.`, classes (`[...]`, `[^...]`, ranges, `\w \W \d \D \s \S` inside), groups (`(...)`, `(?:...)`, `(?=...)`,
//! `(?!...)`, `(?<=...)`, `(?<!...)`), repeats (`* + ? {m} {m,} {m,n} {,n}` and their lazy forms), `|`, anchors
//! (`^ $ \A \Z \b \B`), backreferences `\1`…`\9`, and the flags `(?i)`, `(?m)`, `(?s)` at the start (or given to
//! [`Regex::with_flags`]). An unsupported pattern panics at compile time (the patterns are constants: the tests
//! compile them all).

use super::pystr;

pub const IGNORECASE: u8 = 1;
pub const MULTILINE: u8 = 2;
pub const DOTALL: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Assert {
    /// `^` (MULTILINE: also after a `\n`).
    Bol,
    /// `$`: at the end, or before a `\n` that ends the text (MULTILINE: before any `\n`).
    Eol,
    /// `\A`
    Start,
    /// `\Z`
    End,
    /// `\b`
    Word,
    /// `\B`
    NotWord,
}

#[derive(Debug, Clone, PartialEq)]
enum Item {
    Char(char),
    Range(char, char),
    Word(bool),
    Digit(bool),
    Space(bool),
}

#[derive(Debug, Clone, PartialEq)]
struct Class {
    items: Vec<Item>,
    negated: bool,
}

impl Class {
    fn contains(&self, c: char, ignorecase: bool) -> bool {
        let hit = |c: char| {
            self.items.iter().any(|item| match *item {
                Item::Char(x) => x == c,
                Item::Range(a, b) => a <= c && c <= b,
                Item::Word(yes) => pystr::is_word(c) == yes,
                Item::Digit(yes) => pystr::is_decimal(c) == yes,
                Item::Space(yes) => pystr::is_space(c) == yes,
            })
        };
        let found = hit(c) || (ignorecase && (hit(pystr::lower_char(c)) || hit(pystr::upper_char(c))));
        found != self.negated
    }
}

#[derive(Debug, Clone)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class(Class),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Group(Option<usize>, Box<Node>),
    Repeat { node: Box<Node>, min: usize, max: Option<usize>, greedy: bool },
    Assert(Assert),
    Look { behind: bool, negate: bool, node: Box<Node> },
    Backref(usize),
}

#[derive(Debug, Clone)]
enum Inst {
    Char(char),
    Any,
    Class(usize),
    Split(usize, usize),
    Jmp(usize),
    Save(usize),
    Assert(Assert),
    Backref(usize),
    /// A lookaround: its body runs from `body` (and ends with `LookEnd`); the next instruction jumps over it.
    Look {
        behind: bool,
        negate: bool,
        body: usize,
        width: usize,
    },
    LookEnd,
    Match,
}

/// What may start a match: used to skip start positions quickly.
#[derive(Debug, Clone)]
enum First {
    Char(char),
    Class(usize),
    Any,
}

/// A compiled pattern.
#[derive(Debug, Clone)]
pub struct Regex {
    pattern: String,
    flags: u8,
    groups: usize,
    prog: Vec<Inst>,
    classes: Vec<Class>,
    first: Option<Vec<First>>,
}

/// A match: positions are character indices in the text it was found in.
#[derive(Debug, Clone)]
pub struct Match<'t> {
    text: &'t [char],
    caps: Vec<Option<usize>>,
}

impl<'t> Match<'t> {
    pub fn start(&self) -> usize {
        self.caps[0].unwrap_or(0)
    }

    pub fn end(&self) -> usize {
        self.caps[1].unwrap_or(0)
    }

    /// `m.group(i)`: None for a group that did not take part.
    pub fn get(&self, i: usize) -> Option<String> {
        match (self.caps.get(2 * i).copied().flatten(), self.caps.get(2 * i + 1).copied().flatten()) {
            (Some(a), Some(b)) if a <= b => Some(self.text[a..b].iter().collect()),
            _ => None,
        }
    }

    /// `m.group(i)` of a group that always takes part (else "").
    pub fn group(&self, i: usize) -> String {
        self.get(i).unwrap_or_default()
    }

    /// `m.group(0)`.
    pub fn whole(&self) -> String {
        self.group(0)
    }
}

// -- parsing -------------------------------------------------------------------------------------

struct Parser<'p> {
    chars: Vec<char>,
    pos: usize,
    groups: usize,
    flags: u8,
    pattern: &'p str,
}

impl Parser<'_> {
    fn fail(&self, what: &str) -> ! {
        panic!("pyre: {what} at {} in {:?}", self.pos, self.pattern)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_alt(&mut self) -> Node {
        let mut branches = vec![self.parse_concat()];
        while self.eat('|') {
            branches.push(self.parse_concat());
        }
        if branches.len() == 1 {
            branches.pop().unwrap_or(Node::Empty)
        } else {
            Node::Alt(branches)
        }
    }

    fn parse_concat(&mut self) -> Node {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.parse_atom();
            let atom = self.parse_repeat(atom);
            items.push(atom);
        }
        match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap_or(Node::Empty),
            _ => Node::Concat(items),
        }
    }

    fn parse_number(&mut self) -> Option<usize> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        self.chars[start..self.pos].iter().collect::<String>().parse().ok()
    }

    fn parse_repeat(&mut self, atom: Node) -> Node {
        let mut node = atom;
        loop {
            let (min, max) = match self.peek() {
                Some('*') => {
                    self.pos += 1;
                    (0, None)
                }
                Some('+') => {
                    self.pos += 1;
                    (1, None)
                }
                Some('?') => {
                    self.pos += 1;
                    (0, Some(1))
                }
                Some('{') => {
                    // `{m}`, `{m,}`, `{m,n}`, `{,n}`; anything else is a literal brace (as in Python)
                    let save = self.pos;
                    self.pos += 1;
                    let min = self.parse_number();
                    let (lo, hi) = if self.eat(',') {
                        let max = self.parse_number();
                        (min.unwrap_or(0), max)
                    } else {
                        match min {
                            Some(m) => (m, Some(m)),
                            None => {
                                self.pos = save;
                                return node;
                            }
                        }
                    };
                    if !self.eat('}') {
                        self.pos = save;
                        return node;
                    }
                    (lo, hi)
                }
                _ => return node,
            };
            let greedy = !self.eat('?');
            if matches!(node, Node::Assert(_) | Node::Look { .. }) {
                self.fail("nothing to repeat");
            }
            node = Node::Repeat { node: Box::new(node), min, max, greedy };
        }
    }

    fn parse_escape_char(&mut self) -> char {
        // after the backslash: an escape standing for one character
        let c = self.peek().unwrap_or_else(|| self.fail("trailing backslash"));
        self.pos += 1;
        match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'f' => '\x0c',
            'v' => '\x0b',
            'a' => '\x07',
            'x' => self.hex(2),
            'u' => self.hex(4),
            'U' => self.hex(8),
            c if c.is_ascii_alphanumeric() => self.fail("unknown escape"),
            c => c,
        }
    }

    fn hex(&mut self, n: usize) -> char {
        let end = self.pos + n;
        if end > self.chars.len() {
            self.fail("bad hex escape");
        }
        let digits: String = self.chars[self.pos..end].iter().collect();
        self.pos = end;
        u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32).unwrap_or_else(|| self.fail("bad hex escape"))
    }

    fn parse_atom(&mut self) -> Node {
        let c = self.peek().unwrap_or_else(|| self.fail("unexpected end"));
        self.pos += 1;
        match c {
            '.' => Node::Any,
            '^' => Node::Assert(Assert::Bol),
            '$' => Node::Assert(Assert::Eol),
            '[' => Node::Class(self.parse_class()),
            '(' => self.parse_group(),
            '*' | '+' | '?' => self.fail("nothing to repeat"),
            '\\' => {
                let e = self.peek().unwrap_or_else(|| self.fail("trailing backslash"));
                match e {
                    'w' | 'W' | 'd' | 'D' | 's' | 'S' => {
                        self.pos += 1;
                        let item = class_escape(e);
                        Node::Class(Class { items: vec![item], negated: false })
                    }
                    'b' => {
                        self.pos += 1;
                        Node::Assert(Assert::Word)
                    }
                    'B' => {
                        self.pos += 1;
                        Node::Assert(Assert::NotWord)
                    }
                    'A' => {
                        self.pos += 1;
                        Node::Assert(Assert::Start)
                    }
                    'Z' => {
                        self.pos += 1;
                        Node::Assert(Assert::End)
                    }
                    '1'..='9' => {
                        self.pos += 1;
                        let n = e as usize - '0' as usize;
                        if n > self.groups {
                            self.fail("invalid group reference");
                        }
                        Node::Backref(n)
                    }
                    _ => Node::Char(self.parse_escape_char()),
                }
            }
            c => Node::Char(c),
        }
    }

    fn parse_group(&mut self) -> Node {
        if self.eat('?') {
            let kind = self.peek().unwrap_or_else(|| self.fail("bad group"));
            match kind {
                ':' => {
                    self.pos += 1;
                    let inner = self.parse_alt();
                    self.close();
                    Node::Group(None, Box::new(inner))
                }
                '=' | '!' => {
                    self.pos += 1;
                    let inner = self.parse_alt();
                    self.close();
                    Node::Look { behind: false, negate: kind == '!', node: Box::new(inner) }
                }
                '<' => {
                    self.pos += 1;
                    let kind = self.peek().unwrap_or_else(|| self.fail("bad lookbehind"));
                    if kind != '=' && kind != '!' {
                        self.fail("named groups are not supported");
                    }
                    self.pos += 1;
                    let inner = self.parse_alt();
                    self.close();
                    Node::Look { behind: true, negate: kind == '!', node: Box::new(inner) }
                }
                _ => {
                    // inline flags, at the start of the pattern only: (?i) (?m) (?s), combinations
                    let mut flags = 0;
                    while let Some(f) = self.peek() {
                        match f {
                            'i' => flags |= IGNORECASE,
                            'm' => flags |= MULTILINE,
                            's' => flags |= DOTALL,
                            ')' => break,
                            _ => self.fail("unsupported group"),
                        }
                        self.pos += 1;
                    }
                    self.close();
                    self.flags |= flags;
                    Node::Empty
                }
            }
        } else {
            self.groups += 1;
            let index = self.groups;
            let inner = self.parse_alt();
            self.close();
            Node::Group(Some(index), Box::new(inner))
        }
    }

    fn close(&mut self) {
        if !self.eat(')') {
            self.fail("missing )");
        }
    }

    fn parse_class(&mut self) -> Class {
        let negated = self.eat('^');
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek().unwrap_or_else(|| self.fail("unterminated class"));
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            first = false;
            self.pos += 1;
            let lo = if c == '\\' {
                let e = self.peek().unwrap_or_else(|| self.fail("trailing backslash"));
                if matches!(e, 'w' | 'W' | 'd' | 'D' | 's' | 'S') {
                    self.pos += 1;
                    items.push(class_escape(e));
                    continue;
                }
                if e == 'b' {
                    self.pos += 1;
                    '\x08'
                } else {
                    self.parse_escape_char()
                }
            } else {
                c
            };
            // a range `a-z` (a `-` before `]` is a literal)
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&n| n != ']') {
                self.pos += 1;
                let h = self.peek().unwrap_or_else(|| self.fail("bad range"));
                self.pos += 1;
                let hi = if h == '\\' { self.parse_escape_char() } else { h };
                if hi < lo {
                    self.fail("bad character range");
                }
                items.push(Item::Range(lo, hi));
            } else {
                items.push(Item::Char(lo));
            }
        }
        Class { items, negated }
    }
}

fn class_escape(e: char) -> Item {
    match e {
        'w' => Item::Word(true),
        'W' => Item::Word(false),
        'd' => Item::Digit(true),
        'D' => Item::Digit(false),
        's' => Item::Space(true),
        _ => Item::Space(false),
    }
}

/// (min, max) width in characters; max None: unbounded.
fn width(node: &Node) -> (usize, Option<usize>) {
    match node {
        Node::Empty | Node::Assert(_) | Node::Look { .. } => (0, Some(0)),
        Node::Char(_) | Node::Any | Node::Class(_) => (1, Some(1)),
        Node::Backref(_) => (0, None),
        Node::Group(_, inner) => width(inner),
        Node::Concat(items) => items.iter().fold((0, Some(0)), |(lo, hi), n| {
            let (a, b) = width(n);
            (lo + a, hi.zip(b).map(|(x, y)| x + y))
        }),
        Node::Alt(branches) => {
            let widths: Vec<_> = branches.iter().map(width).collect();
            let lo = widths.iter().map(|w| w.0).min().unwrap_or(0);
            let hi = widths.iter().try_fold(0usize, |m, w| w.1.map(|b| m.max(b)));
            (lo, hi)
        }
        Node::Repeat { node, min, max, .. } => {
            let (a, b) = width(node);
            (a * min, max.and_then(|m| b.map(|b| b * m)))
        }
    }
}

// -- compiling -----------------------------------------------------------------------------------

struct Compiler {
    prog: Vec<Inst>,
    classes: Vec<Class>,
    dotall: bool,
    ignorecase: bool,
}

impl Compiler {
    fn emit(&mut self, inst: Inst) -> usize {
        self.prog.push(inst);
        self.prog.len() - 1
    }

    fn compile(&mut self, node: &Node) {
        match node {
            Node::Empty => {}
            Node::Char(c) => {
                if self.ignorecase && (pystr::lower_char(*c) != *c || pystr::upper_char(*c) != *c) {
                    let class = Class { items: vec![Item::Char(*c)], negated: false };
                    self.classes.push(class);
                    let id = self.classes.len() - 1;
                    self.emit(Inst::Class(id));
                } else {
                    self.emit(Inst::Char(*c));
                }
            }
            Node::Any => {
                if self.dotall {
                    self.emit(Inst::Any);
                } else {
                    self.classes.push(Class { items: vec![Item::Char('\n')], negated: true });
                    let id = self.classes.len() - 1;
                    self.emit(Inst::Class(id));
                }
            }
            Node::Class(class) => {
                self.classes.push(class.clone());
                let id = self.classes.len() - 1;
                self.emit(Inst::Class(id));
            }
            Node::Concat(items) => items.iter().for_each(|n| self.compile(n)),
            Node::Alt(branches) => {
                let mut jumps = Vec::new();
                for (i, branch) in branches.iter().enumerate() {
                    if i + 1 < branches.len() {
                        let split = self.emit(Inst::Split(0, 0));
                        self.compile(branch);
                        jumps.push(self.emit(Inst::Jmp(0)));
                        let next = self.prog.len();
                        self.prog[split] = Inst::Split(split + 1, next);
                    } else {
                        self.compile(branch);
                    }
                }
                let end = self.prog.len();
                for j in jumps {
                    self.prog[j] = Inst::Jmp(end);
                }
            }
            Node::Group(index, inner) => match index {
                Some(i) => {
                    self.emit(Inst::Save(2 * i));
                    self.compile(inner);
                    self.emit(Inst::Save(2 * i + 1));
                }
                None => self.compile(inner),
            },
            Node::Repeat { node, min, max, greedy } => {
                if max.is_none() && width(node).0 == 0 {
                    panic!("pyre: a repeat of something that may be empty is not supported");
                }
                for _ in 0..*min {
                    self.compile(node);
                }
                match max {
                    None => {
                        // L1: split(L2, L3); L2: node; jmp L1; L3:
                        let split = self.emit(Inst::Split(0, 0));
                        self.compile(node);
                        self.emit(Inst::Jmp(split));
                        let end = self.prog.len();
                        self.prog[split] =
                            if *greedy { Inst::Split(split + 1, end) } else { Inst::Split(end, split + 1) };
                    }
                    Some(max) => {
                        // nested optionals: (node (node (node)?)?)?
                        let mut splits = Vec::new();
                        for _ in *min..*max {
                            splits.push(self.emit(Inst::Split(0, 0)));
                            self.compile(node);
                        }
                        let end = self.prog.len();
                        for split in splits {
                            self.prog[split] =
                                if *greedy { Inst::Split(split + 1, end) } else { Inst::Split(end, split + 1) };
                        }
                    }
                }
            }
            Node::Assert(a) => {
                self.emit(Inst::Assert(*a));
            }
            Node::Backref(n) => {
                self.emit(Inst::Backref(*n));
            }
            Node::Look { behind, negate, node } => {
                let width = if *behind {
                    match width(node) {
                        (lo, Some(hi)) if lo == hi => lo,
                        _ => panic!("pyre: look-behind requires fixed-width pattern"),
                    }
                } else {
                    0
                };
                let look = self.emit(Inst::Look { behind: *behind, negate: *negate, body: 0, width });
                let jump = self.emit(Inst::Jmp(0));
                let body = self.prog.len();
                self.compile(node);
                self.emit(Inst::LookEnd);
                let end = self.prog.len();
                self.prog[look] = Inst::Look { behind: *behind, negate: *negate, body, width };
                self.prog[jump] = Inst::Jmp(end);
            }
        }
    }

    /// The characters a match may start with (None: anything, or an empty match).
    fn first(&mut self, node: &Node) -> (Option<Vec<First>>, bool) {
        match node {
            Node::Empty | Node::Assert(_) | Node::Look { .. } => (Some(Vec::new()), true),
            Node::Char(c) => {
                if self.ignorecase && (pystr::lower_char(*c) != *c || pystr::upper_char(*c) != *c) {
                    self.classes.push(Class { items: vec![Item::Char(*c)], negated: false });
                    (Some(vec![First::Class(self.classes.len() - 1)]), false)
                } else {
                    (Some(vec![First::Char(*c)]), false)
                }
            }
            Node::Any => (Some(vec![First::Any]), false),
            Node::Class(class) => {
                self.classes.push(class.clone());
                (Some(vec![First::Class(self.classes.len() - 1)]), false)
            }
            Node::Backref(_) => (None, true),
            Node::Group(_, inner) => self.first(inner),
            Node::Concat(items) => {
                let mut out = Vec::new();
                for item in items {
                    let (set, nullable) = self.first(item);
                    match set {
                        None => return (None, true),
                        Some(s) => out.extend(s),
                    }
                    if !nullable {
                        return (Some(out), false);
                    }
                }
                (Some(out), true)
            }
            Node::Alt(branches) => {
                let mut out = Vec::new();
                let mut any_nullable = false;
                for branch in branches {
                    let (set, nullable) = self.first(branch);
                    match set {
                        None => return (None, true),
                        Some(s) => out.extend(s),
                    }
                    any_nullable |= nullable;
                }
                (Some(out), any_nullable)
            }
            Node::Repeat { node, min, .. } => {
                let (set, nullable) = self.first(node);
                (set, nullable || *min == 0)
            }
        }
    }
}

impl Regex {
    pub fn new(pattern: &str) -> Regex {
        Regex::with_flags(pattern, 0)
    }

    pub fn with_flags(pattern: &str, flags: u8) -> Regex {
        let mut parser = Parser { chars: pattern.chars().collect(), pos: 0, groups: 0, flags, pattern };
        let node = parser.parse_alt();
        if parser.pos != parser.chars.len() {
            parser.fail("unbalanced parenthesis");
        }
        let flags = parser.flags;
        let mut compiler = Compiler {
            prog: Vec::new(),
            classes: Vec::new(),
            dotall: flags & DOTALL != 0,
            ignorecase: flags & IGNORECASE != 0,
        };
        compiler.compile(&node);
        compiler.emit(Inst::Match);
        let first = match compiler.first(&node) {
            (Some(set), false) => Some(set),
            _ => None,
        };
        Regex {
            pattern: pattern.to_string(),
            flags,
            groups: parser.groups,
            prog: compiler.prog,
            classes: compiler.classes,
            first,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    pub fn groups(&self) -> usize {
        self.groups
    }

    fn ignorecase(&self) -> bool {
        self.flags & IGNORECASE != 0
    }

    fn may_start(&self, c: char) -> bool {
        match &self.first {
            None => true,
            Some(set) => set.iter().any(|f| match f {
                First::Char(x) => *x == c,
                First::Class(id) => self.classes[*id].contains(c, self.ignorecase()),
                First::Any => true,
            }),
        }
    }

    fn assert(&self, a: Assert, text: &[char], pos: usize) -> bool {
        let multiline = self.flags & MULTILINE != 0;
        let n = text.len();
        match a {
            Assert::Bol => pos == 0 || (multiline && text[pos - 1] == '\n'),
            Assert::Eol => pos == n || (multiline && text[pos] == '\n') || (pos + 1 == n && text[pos] == '\n'),
            Assert::Start => pos == 0,
            Assert::End => pos == n,
            Assert::Word | Assert::NotWord => {
                if n == 0 {
                    return false; // sre: no boundary (nor non-boundary) in an empty string
                }
                let before = pos > 0 && pystr::is_word(text[pos - 1]);
                let after = pos < n && pystr::is_word(text[pos]);
                (before != after) == (a == Assert::Word)
            }
        }
    }

    /// Runs the program from `pc` at `pos`; on success `caps` holds the captures and the end is returned.
    /// `forbid_empty_at`: a match may not end where it started at that position (Python's `must_advance`).
    /// `require_end`: the match must end there (`fullmatch`).
    fn run(
        &self,
        text: &[char],
        pc: usize,
        pos: usize,
        caps: &mut [Option<usize>],
        forbid_empty_at: Option<usize>,
        require_end: Option<usize>,
    ) -> Option<usize> {
        enum Frame {
            Try(usize, usize),
            Restore(usize, Option<usize>),
        }
        let start = pos;
        let ignorecase = self.ignorecase();
        let mut stack: Vec<Frame> = Vec::new();
        let (mut pc, mut pos) = (pc, pos);
        loop {
            let ok = match &self.prog[pc] {
                Inst::Char(c) => {
                    if pos < text.len() && text[pos] == *c {
                        pos += 1;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Any => {
                    if pos < text.len() {
                        pos += 1;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Class(id) => {
                    if pos < text.len() && self.classes[*id].contains(text[pos], ignorecase) {
                        pos += 1;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Split(a, b) => {
                    stack.push(Frame::Try(*b, pos));
                    pc = *a;
                    true
                }
                Inst::Jmp(to) => {
                    pc = *to;
                    true
                }
                Inst::Save(slot) => {
                    stack.push(Frame::Restore(*slot, caps[*slot]));
                    caps[*slot] = Some(pos);
                    pc += 1;
                    true
                }
                Inst::Assert(a) => {
                    if self.assert(*a, text, pos) {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Backref(n) => match (caps[2 * n], caps[2 * n + 1]) {
                    (Some(a), Some(b)) if a <= b => {
                        let len = b - a;
                        let same = pos + len <= text.len()
                            && (0..len).all(|i| {
                                let (x, y) = (text[a + i], text[pos + i]);
                                x == y || (ignorecase && pystr::lower_char(x) == pystr::lower_char(y))
                            });
                        if same {
                            pos += len;
                            pc += 1;
                        }
                        same
                    }
                    _ => false,
                },
                Inst::Look { behind, negate, body, width } => {
                    let mut inner: Vec<Option<usize>> = caps.to_vec();
                    let found = if *behind {
                        pos >= *width && self.run(text, *body, pos - width, &mut inner, None, None).is_some()
                    } else {
                        self.run(text, *body, pos, &mut inner, None, None).is_some()
                    };
                    if found != *negate {
                        if !*negate {
                            // a positive lookaround keeps its captures (as sre does)
                            for (slot, value) in inner.into_iter().enumerate() {
                                if caps[slot] != value {
                                    stack.push(Frame::Restore(slot, caps[slot]));
                                    caps[slot] = value;
                                }
                            }
                        }
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::LookEnd => return Some(pos),
                Inst::Match => {
                    if (forbid_empty_at == Some(start) && pos == start) || require_end.is_some_and(|e| e != pos) {
                        false
                    } else {
                        return Some(pos);
                    }
                }
            };
            if ok {
                continue;
            }
            // backtrack
            loop {
                match stack.pop() {
                    None => return None,
                    Some(Frame::Restore(slot, value)) => caps[slot] = value,
                    Some(Frame::Try(p, at)) => {
                        pc = p;
                        pos = at;
                        break;
                    }
                }
            }
        }
    }

    fn match_here<'t>(&self, text: &'t [char], pos: usize, forbid_empty_at: Option<usize>) -> Option<Match<'t>> {
        self.match_until(text, pos, forbid_empty_at, None)
    }

    fn match_until<'t>(
        &self,
        text: &'t [char],
        pos: usize,
        forbid_empty_at: Option<usize>,
        end: Option<usize>,
    ) -> Option<Match<'t>> {
        let mut caps = vec![None; 2 * (self.groups + 1)];
        let end = self.run(text, 0, pos, &mut caps, forbid_empty_at, end)?;
        caps[0] = Some(pos);
        caps[1] = Some(end);
        Some(Match { text, caps })
    }

    fn search_from<'t>(&self, text: &'t [char], from: usize, forbid_empty_at: Option<usize>) -> Option<Match<'t>> {
        let n = text.len();
        let mut pos = from;
        while pos <= n {
            if self.first.is_none() || (pos < n && self.may_start(text[pos])) {
                if let Some(m) = self.match_here(text, pos, forbid_empty_at) {
                    return Some(m);
                }
            }
            pos += 1;
        }
        None
    }

    /// `pattern.search(text)` on characters.
    pub fn search_chars<'t>(&self, text: &'t [char]) -> Option<Match<'t>> {
        self.search_from(text, 0, None)
    }

    /// `pattern.match(text)` (anchored at the start) on characters.
    pub fn match_chars<'t>(&self, text: &'t [char]) -> Option<Match<'t>> {
        self.match_here(text, 0, None)
    }

    /// `bool(pattern.search(text))`.
    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        self.search_chars(&chars).is_some()
    }

    /// `bool(pattern.match(text))`.
    pub fn matches_at_start(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        self.match_chars(&chars).is_some()
    }

    /// `pattern.fullmatch(text)`: the groups (index 0: the whole text), or None.
    pub fn fullmatch(&self, text: &str) -> Option<Vec<Option<String>>> {
        let chars: Vec<char> = text.chars().collect();
        let m = self.match_until(&chars, 0, None, Some(chars.len()))?;
        Some((0..=self.groups).map(|i| m.get(i)).collect())
    }

    /// The groups of the first match (`pattern.search(text)`), index 0 the whole match.
    pub fn captures(&self, text: &str) -> Option<Vec<Option<String>>> {
        let chars: Vec<char> = text.chars().collect();
        let m = self.search_chars(&chars)?;
        Some((0..=self.groups).map(|i| m.get(i)).collect())
    }

    /// Every match, as Python's `finditer` (empty matches handled as `re.sub` does).
    pub fn find_iter<'t>(&self, text: &'t [char]) -> Vec<Match<'t>> {
        let mut out = Vec::new();
        let mut pos = 0;
        let mut must_advance = false;
        while pos <= text.len() {
            let Some(m) = self.search_from(text, pos, must_advance.then_some(pos)) else { break };
            must_advance = m.end() == m.start();
            pos = m.end();
            out.push(m);
        }
        out
    }

    /// `pattern.sub(function, text)`.
    pub fn replace_with(&self, text: &str, mut replace: impl FnMut(&Match) -> String) -> String {
        let chars: Vec<char> = text.chars().collect();
        let matches = self.find_iter(&chars);
        if matches.is_empty() {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for m in &matches {
            out.extend(&chars[last..m.start()]);
            out.push_str(&replace(m));
            last = m.end();
        }
        out.extend(&chars[last..]);
        out
    }

    /// `pattern.sub(template, text)`: `\1`, `\g<1>` and the usual escapes in the template.
    pub fn replace(&self, text: &str, template: &str) -> String {
        let parts = parse_template(template);
        if parts.iter().all(|p| matches!(p, Piece::Text(_))) {
            let literal: String = parts.iter().map(|p| if let Piece::Text(t) = p { t.as_str() } else { "" }).collect();
            return self.replace_with(text, |_| literal.clone());
        }
        self.replace_with(text, |m| {
            parts
                .iter()
                .map(|p| match p {
                    Piece::Text(t) => t.clone(),
                    Piece::Group(i) => m.group(*i),
                })
                .collect()
        })
    }

    /// `pattern.split(text)` (a pattern without capturing groups).
    pub fn split(&self, text: &str) -> Vec<String> {
        assert_eq!(self.groups, 0, "pyre: split with capturing groups is not supported");
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut last = 0;
        for m in self.find_iter(&chars) {
            out.push(chars[last..m.start()].iter().collect());
            last = m.end();
        }
        out.push(chars[last..].iter().collect());
        out
    }
}

enum Piece {
    Text(String),
    Group(usize),
}

fn parse_template(template: &str) -> Vec<Piece> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            text.push(c);
            i += 1;
            continue;
        }
        let e = chars[i + 1];
        i += 2;
        let group = match e {
            '0'..='9' => {
                let mut n = e as usize - '0' as usize;
                if i < chars.len() && chars[i].is_ascii_digit() {
                    n = n * 10 + (chars[i] as usize - '0' as usize);
                    i += 1;
                }
                Some(n)
            }
            'g' if i < chars.len() && chars[i] == '<' => {
                let close = chars[i..].iter().position(|&c| c == '>').map(|p| p + i).expect("pyre: bad \\g<>");
                let n: usize =
                    chars[i + 1..close].iter().collect::<String>().parse().expect("pyre: named groups unsupported");
                i = close + 1;
                Some(n)
            }
            _ => {
                match e {
                    'n' => text.push('\n'),
                    't' => text.push('\t'),
                    'r' => text.push('\r'),
                    '\\' => text.push('\\'),
                    other => {
                        text.push('\\');
                        text.push(other);
                    }
                }
                None
            }
        };
        if let Some(n) = group {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut text)));
            }
            out.push(Piece::Group(n));
        }
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// `re.escape(text)` (Python 3.7+: only the characters special in a pattern, each after a backslash).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(p: &str, t: &str, r: &str) -> String {
        Regex::new(p).replace(t, r)
    }

    #[test]
    fn basics() {
        assert_eq!(sub(r"\d+", "a12b3", "#"), "a#b#");
        assert_eq!(sub(r"(\*\*|__)(.+?)\1", "**gras** et __x__", r"\2"), "gras et x");
        assert_eq!(sub(r"x*", "abxd", "-"), "-a-b--d-");
        assert_eq!(sub(r"(?<=[a-z])(?=[A-Z])", "DuckDuckGo", " "), "Duck Duck Go");
        assert_eq!(sub(r"km\b", "km² km", "K"), "km² K"); // ² is a word character in Python
        assert!(Regex::new(r"^\s*([-*_=]\s*){3,}$").matches_at_start("- - -"));
        assert!(Regex::new(r"^a$").matches_at_start("a\n"));
        assert!(!Regex::new(r"^a$").matches_at_start("a\n\n"));
        assert_eq!(sub(r"(?m)^[ \t]*-[ \t]*", "x\n - y\n-z", ""), "x\ny\nz");
        assert_eq!(Regex::with_flags(r"\bRUN\b", IGNORECASE).replace("les runs, le run", "R"), "les runs, le R");
        let m = Regex::new(r"(?<![\w.,])1 (?=([^\W\d]+))").captures("1 día").unwrap();
        assert_eq!(m[1].as_deref(), Some("día"));
        assert_eq!(Regex::new(r"(-?)(\d+)(?:[,.](\d+))?").fullmatch("3,5").unwrap()[3].as_deref(), Some("5"));
        assert!(Regex::new(r"(-?)(\d+)(?:[,.](\d+))?").fullmatch("3,5x").is_none());
        assert_eq!(Regex::new(r"\n\s*\n").split("a\n\nb\n \nc"), vec!["a", "b", "c"]);
        assert_eq!(sub(r"```.*?(?:```|$)", "a```x\n", ""), "a\n"); // `$` before the final newline
        assert_eq!(Regex::with_flags(r"```.*?(?:```|$)", DOTALL).replace("a```x\n", ""), "a\n");
        assert_eq!(sub(r"[^\W\d]+", "ab1 é_", "W"), "W1 W");
        assert_eq!(escape("Mme. Dupuis (x)"), r"Mme\.\ Dupuis\ \(x\)");
        assert!(Regex::new(&escape("a.b c\t(d)")).fullmatch("a.b c\t(d)").is_some());
    }
}
