//! MQL, the issue query language: `project = PAY AND status != done ORDER BY updated DESC`.
//!
//! A query is lexed and parsed into a small tree, checked against a closed list of fields,
//! operators and functions, and compiled into an OpenSearch query built entirely here.
//! Nothing a person types reaches OpenSearch as query syntax: values only ever become term
//! values, analysed match text, or date math assembled from validated parts. The tenant
//! and project-visibility filters are added by the caller around the compiled query and
//! cannot be widened by it.
//!
//! Dates are in UTC.

use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

pub const MAX_QUERY_CHARS: usize = 2000;
const MAX_CLAUSES: usize = 100;
const MAX_DEPTH: usize = 16;
const MAX_LIST: usize = 100;
const MAX_VALUE_CHARS: usize = 200;
const MAX_ORDER: usize = 3;

/// What went wrong and where: `[start, end)` in characters of the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqlError {
    pub message: String,
    pub start: usize,
    pub end: usize,
}

type Result<T> = std::result::Result<T, MqlError>;

fn fail<T>(message: impl Into<String>, start: usize, end: usize) -> Result<T> {
    Err(MqlError { message: message.into(), start, end })
}

// ---------------------------------------------------------------------------------------
// Lexing

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || "()=,!<>~\"'".contains(c)
}

fn lex(input: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_control() {
            return fail("This character cannot be used in a query.", i, i + 1);
        }
        let start = i;
        let next = chars.get(i + 1).copied();
        let (tok, len) = match c {
            '(' => (Tok::LParen, 1),
            ')' => (Tok::RParen, 1),
            ',' => (Tok::Comma, 1),
            '=' => (Tok::Op("="), 1),
            '~' => (Tok::Op("~"), 1),
            '!' if next == Some('=') => (Tok::Op("!="), 2),
            '!' if next == Some('~') => (Tok::Op("!~"), 2),
            '!' => return fail("Use != (is not) or !~ (does not contain).", i, i + 1),
            '<' if next == Some('=') => (Tok::Op("<="), 2),
            '<' => (Tok::Op("<"), 1),
            '>' if next == Some('=') => (Tok::Op(">="), 2),
            '>' => (Tok::Op(">"), 1),
            '"' | '\'' => {
                let mut text = String::new();
                let mut j = i + 1;
                loop {
                    match chars.get(j) {
                        None => return fail("This text is missing its closing quote.", start, chars.len()),
                        Some('\\') if j + 1 < chars.len() => {
                            text.push(chars[j + 1]);
                            j += 2;
                        }
                        Some(&q) if q == c => break,
                        Some(&d) if d.is_control() => return fail("This character cannot be used in a query.", j, j + 1),
                        Some(&d) => {
                            text.push(d);
                            j += 1;
                        }
                    }
                }
                (Tok::Str(text), j + 1 - i)
            }
            _ => {
                let mut j = i;
                while j < chars.len() && !is_delimiter(chars[j]) && !chars[j].is_control() {
                    j += 1;
                }
                (Tok::Word(chars[i..j].iter().collect()), j - i)
            }
        };
        i += len;
        out.push(Token { tok, start, end: i });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Syntax tree

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Project,
    Key,
    Type,
    Status,
    StatusCategory,
    Priority,
    Assignee,
    Reporter,
    Labels,
    Summary,
    Description,
    Comment,
    Text,
    Created,
    Updated,
    Resolved,
}

/// Field names people can type, lower-case, with what they mean. The first name of each
/// field is the one shown in messages.
pub const FIELD_NAMES: &[(&str, Field)] = &[
    ("project", Field::Project),
    ("key", Field::Key),
    ("issue", Field::Key),
    ("issuekey", Field::Key),
    ("type", Field::Type),
    ("issuetype", Field::Type),
    ("status", Field::Status),
    ("statuscategory", Field::StatusCategory),
    ("category", Field::StatusCategory),
    ("priority", Field::Priority),
    ("assignee", Field::Assignee),
    ("reporter", Field::Reporter),
    ("labels", Field::Labels),
    ("label", Field::Labels),
    ("summary", Field::Summary),
    ("description", Field::Description),
    ("comment", Field::Comment),
    ("comments", Field::Comment),
    ("text", Field::Text),
    ("created", Field::Created),
    ("updated", Field::Updated),
    ("resolved", Field::Resolved),
];

impl Field {
    fn parse(word: &str) -> Option<Self> {
        let lower = word.to_ascii_lowercase();
        FIELD_NAMES.iter().find(|(name, _)| *name == lower).map(|(_, f)| *f)
    }

    pub fn name(self) -> &'static str {
        match self {
            Field::StatusCategory => "statusCategory",
            f => FIELD_NAMES.iter().find(|(_, g)| *g == f).map(|(n, _)| *n).unwrap_or("field"),
        }
    }

    fn allows(self, op: Op) -> bool {
        use Op::*;
        match self {
            Field::Project | Field::Key | Field::Type | Field::Status | Field::StatusCategory => {
                matches!(op, Eq | Ne | In | NotIn)
            }
            Field::Priority => matches!(op, Eq | Ne | In | NotIn | Lt | Le | Gt | Ge),
            Field::Assignee | Field::Reporter | Field::Labels => matches!(op, Eq | Ne | In | NotIn | IsEmpty | IsNotEmpty),
            Field::Summary | Field::Description | Field::Comment | Field::Text => matches!(op, Contains | NotContains),
            Field::Created | Field::Updated => matches!(op, Eq | Lt | Le | Gt | Ge),
            Field::Resolved => matches!(op, Eq | Lt | Le | Gt | Ge | IsEmpty | IsNotEmpty),
        }
    }

    fn operators_help(self) -> &'static str {
        match self {
            Field::Project | Field::Key | Field::Type | Field::Status | Field::StatusCategory => "=, !=, IN and NOT IN",
            Field::Priority => "=, !=, IN, NOT IN, <, <=, > and >=",
            Field::Assignee | Field::Reporter | Field::Labels => "=, !=, IN, NOT IN, IS EMPTY and IS NOT EMPTY",
            Field::Summary | Field::Description | Field::Comment | Field::Text => "~ (contains) and !~ (does not contain)",
            Field::Created | Field::Updated => "=, <, <=, > and >=",
            Field::Resolved => "=, <, <=, >, >=, IS EMPTY and IS NOT EMPTY",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    NotContains,
    In,
    NotIn,
    IsEmpty,
    IsNotEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Val {
    Text(String),
    /// Lower-cased function name, e.g. `currentuser`.
    Function(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operand {
    pub value: Val,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    pub field: Field,
    pub op: Op,
    pub values: Vec<Operand>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Clause(Clause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Created,
    Updated,
    Resolved,
    Priority,
    Key,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub filter: Option<Expr>,
    /// (field, descending)
    pub order: Vec<(SortField, bool)>,
}

// ---------------------------------------------------------------------------------------
// Parsing

const KEYWORDS: &[&str] = &["and", "or", "not", "in", "is", "empty", "null", "order", "by", "asc", "desc"];

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    len: usize,
    clauses: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        matches!(self.peek(), Some(Token { tok: Tok::Word(w), .. }) if w.eq_ignore_ascii_case(keyword))
    }

    fn eat_keyword(&mut self, keyword: &str) -> bool {
        let found = self.peek_keyword(keyword);
        if found {
            self.pos += 1;
        }
        found
    }

    /// Span to blame when something is missing: the next token, or the end of the query.
    fn here(&self) -> (usize, usize) {
        match self.peek() {
            Some(t) => (t.start, t.end),
            None => (self.len, self.len),
        }
    }

    fn query(&mut self) -> Result<Query> {
        let filter = if self.peek().is_none() || self.peek_keyword("order") { None } else { Some(self.or(0)?) };
        let mut order = Vec::new();
        if self.eat_keyword("order") {
            if !self.eat_keyword("by") {
                let (s, e) = self.here();
                return fail("Expected BY after ORDER.", s, e);
            }
            loop {
                let (start, end) = self.here();
                let field = match self.peek() {
                    Some(Token { tok: Tok::Word(w), .. }) => match w.to_ascii_lowercase().as_str() {
                        "created" => SortField::Created,
                        "updated" => SortField::Updated,
                        "resolved" => SortField::Resolved,
                        "priority" => SortField::Priority,
                        "key" | "issue" | "issuekey" => SortField::Key,
                        _ => return fail("Sort by created, updated, resolved, priority or key.", start, end),
                    },
                    _ => return fail("Expected a field to sort by.", start, end),
                };
                self.pos += 1;
                if order.iter().any(|(f, _)| *f == field) {
                    return fail("This field is already in ORDER BY.", start, end);
                }
                let descending = if self.eat_keyword("desc") {
                    true
                } else {
                    self.eat_keyword("asc");
                    false
                };
                order.push((field, descending));
                if order.len() > MAX_ORDER {
                    return fail("Sort by at most three fields.", start, end);
                }
                if matches!(self.peek(), Some(Token { tok: Tok::Comma, .. })) {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if let Some(t) = self.peek() {
            let message = if filter.is_some() && order.is_empty() {
                "Expected AND, OR or ORDER BY here."
            } else {
                "Unexpected text at the end of the query."
            };
            return fail(message, t.start, t.end);
        }
        Ok(Query { filter, order })
    }

    fn deeper(&self, depth: usize) -> Result<usize> {
        if depth >= MAX_DEPTH {
            let (s, e) = self.here();
            return fail("The query is nested too deeply.", s, e);
        }
        Ok(depth + 1)
    }

    fn or(&mut self, depth: usize) -> Result<Expr> {
        let depth = self.deeper(depth)?;
        let mut parts = vec![self.and(depth)?];
        while self.eat_keyword("or") {
            parts.push(self.and(depth)?);
        }
        Ok(if parts.len() == 1 { parts.remove(0) } else { Expr::Or(parts) })
    }

    fn and(&mut self, depth: usize) -> Result<Expr> {
        let mut parts = vec![self.not(depth)?];
        while self.eat_keyword("and") {
            parts.push(self.not(depth)?);
        }
        Ok(if parts.len() == 1 { parts.remove(0) } else { Expr::And(parts) })
    }

    fn not(&mut self, depth: usize) -> Result<Expr> {
        if self.eat_keyword("not") {
            let depth = self.deeper(depth)?;
            return Ok(Expr::Not(Box::new(self.not(depth)?)));
        }
        match self.peek() {
            Some(Token { tok: Tok::LParen, .. }) => {
                self.pos += 1;
                let inner = self.or(depth)?;
                match self.peek() {
                    Some(Token { tok: Tok::RParen, .. }) => {
                        self.pos += 1;
                        Ok(inner)
                    }
                    _ => {
                        let (s, e) = self.here();
                        fail("Expected a closing parenthesis.", s, e)
                    }
                }
            }
            _ => self.clause().map(Expr::Clause),
        }
    }

    fn clause(&mut self) -> Result<Clause> {
        let (start, end) = self.here();
        let field = match self.peek() {
            Some(Token { tok: Tok::Word(w), .. }) => match Field::parse(w) {
                Some(f) => f,
                None if KEYWORDS.contains(&w.to_ascii_lowercase().as_str()) => {
                    return fail("Expected a field here, such as project, status or assignee.", start, end)
                }
                None => {
                    return fail(format!("Unknown field \"{w}\". Try project, status, assignee, text or created."), start, end)
                }
            },
            Some(Token { tok: Tok::Str(_), .. }) => {
                return fail("Field names are not quoted. Custom fields cannot be searched yet.", start, end)
            }
            _ => return fail("Expected a field here, such as project, status or assignee.", start, end),
        };
        self.pos += 1;
        self.clauses += 1;
        if self.clauses > MAX_CLAUSES {
            return fail("The query has too many conditions.", start, end);
        }

        let (op_start, op_end) = self.here();
        let op = match self.peek().map(|t| t.tok.clone()) {
            Some(Tok::Op(o)) => {
                self.pos += 1;
                match o {
                    "=" => Op::Eq,
                    "!=" => Op::Ne,
                    "<" => Op::Lt,
                    "<=" => Op::Le,
                    ">" => Op::Gt,
                    ">=" => Op::Ge,
                    "~" => Op::Contains,
                    _ => Op::NotContains,
                }
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("in") => {
                self.pos += 1;
                Op::In
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("not") => {
                self.pos += 1;
                if !self.eat_keyword("in") {
                    let (s, e) = self.here();
                    return fail("Expected IN after NOT.", s, e);
                }
                Op::NotIn
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("is") => {
                self.pos += 1;
                let negated = self.eat_keyword("not");
                if !(self.eat_keyword("empty") || self.eat_keyword("null")) {
                    let (s, e) = self.here();
                    return fail("Expected EMPTY after IS.", s, e);
                }
                if negated {
                    Op::IsNotEmpty
                } else {
                    Op::IsEmpty
                }
            }
            _ => return fail("Expected an operator such as =, !=, ~ or IN.", op_start, op_end),
        };

        // `= EMPTY` and `!= EMPTY` read like IS [NOT] EMPTY.
        let op = match op {
            Op::Eq | Op::Ne if self.peek_keyword("empty") || self.peek_keyword("null") => {
                self.pos += 1;
                if op == Op::Eq {
                    Op::IsEmpty
                } else {
                    Op::IsNotEmpty
                }
            }
            op => op,
        };
        let op_end = self.tokens.get(self.pos.saturating_sub(1)).map_or(op_end, |t| t.end);
        if !field.allows(op) {
            return fail(format!("{} supports {}.", field.name(), field.operators_help()), op_start, op_end);
        }

        let values = match op {
            Op::IsEmpty | Op::IsNotEmpty => vec![],
            Op::In | Op::NotIn => self.list()?,
            _ => vec![self.operand()?],
        };
        let end = values.last().map_or(op_end, |v| v.end);
        Ok(Clause { field, op, values, start, end })
    }

    fn list(&mut self) -> Result<Vec<Operand>> {
        if !matches!(self.peek(), Some(Token { tok: Tok::LParen, .. })) {
            let (s, e) = self.here();
            return fail("Put the values in parentheses: IN (a, b).", s, e);
        }
        self.pos += 1;
        let mut values = vec![self.operand()?];
        loop {
            match self.peek().map(|t| t.tok.clone()) {
                Some(Tok::Comma) => {
                    self.pos += 1;
                    values.push(self.operand()?);
                    if values.len() > MAX_LIST {
                        let last = values.last().expect("just pushed");
                        return fail("A list can hold at most 100 values.", last.start, last.end);
                    }
                }
                Some(Tok::RParen) => {
                    self.pos += 1;
                    return Ok(values);
                }
                _ => {
                    let (s, e) = self.here();
                    return fail("Expected a comma or a closing parenthesis.", s, e);
                }
            }
        }
    }

    fn operand(&mut self) -> Result<Operand> {
        let (start, end) = self.here();
        let value = match self.peek().map(|t| t.tok.clone()) {
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Val::Text(s)
            }
            Some(Tok::Word(w)) if KEYWORDS.contains(&w.to_ascii_lowercase().as_str()) => {
                return fail("Expected a value here. Quote it if it is a word like \"and\".", start, end)
            }
            Some(Tok::Word(w)) => {
                self.pos += 1;
                if matches!(self.peek(), Some(Token { tok: Tok::LParen, .. })) {
                    self.pos += 1;
                    if !matches!(self.peek(), Some(Token { tok: Tok::RParen, .. })) {
                        let (s, e) = self.here();
                        return fail("Functions take no arguments: write currentUser().", s, e);
                    }
                    self.pos += 1;
                    let end = self.tokens[self.pos - 1].end;
                    return Ok(Operand { value: Val::Function(w.to_ascii_lowercase()), start, end });
                }
                Val::Text(w)
            }
            _ => return fail("Expected a value here.", start, end),
        };
        if let Val::Text(t) = &value {
            if t.chars().count() > MAX_VALUE_CHARS {
                return fail("Values can be at most 200 characters.", start, end);
            }
        }
        Ok(Operand { value, start, end })
    }
}

pub fn parse(input: &str) -> Result<Query> {
    let len = input.chars().count();
    if len > MAX_QUERY_CHARS {
        return fail("Queries can be at most 2000 characters.", MAX_QUERY_CHARS, len);
    }
    let tokens = lex(input)?;
    Parser { tokens, pos: 0, len, clauses: 0 }.query()
}

// ---------------------------------------------------------------------------------------
// Compiling

/// People named by email or display name in a query, as typed (for [`Context::people`]).
pub fn person_terms(query: &Query) -> Vec<String> {
    fn walk(expr: &Expr, out: &mut Vec<String>) {
        match expr {
            Expr::And(parts) | Expr::Or(parts) => parts.iter().for_each(|p| walk(p, out)),
            Expr::Not(inner) => walk(inner, out),
            Expr::Clause(c) if matches!(c.field, Field::Assignee | Field::Reporter) => {
                for v in &c.values {
                    if let Val::Text(t) = &v.value {
                        if Uuid::parse_str(t.trim()).is_err() {
                            out.push(person_key(t));
                        }
                    }
                }
            }
            Expr::Clause(_) => {}
        }
    }
    let mut out = Vec::new();
    if let Some(filter) = &query.filter {
        walk(filter, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// How a person term is looked up: trimmed, lower-case.
pub fn person_key(term: &str) -> String {
    term.trim().to_lowercase()
}

pub struct Context<'a> {
    pub current_user: Uuid,
    /// [`person_key`] → the members it names. `None` when the caller may not look people
    /// up by name or email (guests), so only `currentUser()` and ids work.
    pub people: Option<&'a HashMap<String, Vec<Uuid>>>,
}

/// What a sort value at one position of a cursor must look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortValue {
    Number,
    Text,
}

#[derive(Debug, Clone)]
pub struct Compiled {
    /// Goes under the tenant and visibility filters.
    pub query: Value,
    pub sort: Value,
    pub sort_values: Vec<SortValue>,
}

/// `lowest` … `highest` as 1 … 5 (stored as `priority_rank`).
pub fn priority_rank(priority: &str) -> Option<i32> {
    match priority {
        "lowest" => Some(1),
        "low" => Some(2),
        "medium" => Some(3),
        "high" => Some(4),
        "highest" => Some(5),
        _ => None,
    }
}

/// `In Review` → `in_review`.
fn slug(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn text(op: &Operand) -> Result<&str> {
    match &op.value {
        Val::Text(t) => Ok(t),
        Val::Function(f) => fail(format!("{f}() cannot be used here."), op.start, op.end),
    }
}

fn status_category(op: &Operand) -> Result<&'static str> {
    match slug(text(op)?).as_str() {
        "todo" | "to_do" | "new" => Ok("todo"),
        "in_progress" | "indeterminate" => Ok("in_progress"),
        "done" | "complete" => Ok("done"),
        _ => fail("statusCategory is \"To Do\", \"In Progress\" or \"Done\".", op.start, op.end),
    }
}

/// A date the way OpenSearch should compare it.
enum DateValue {
    /// A calendar day, `2026-11-01`.
    Day(String),
    /// A minute, `2026-11-01T09:30:00Z`.
    Minute(String),
    /// Date math relative to now, assembled here (`now-7d`, `now/w`).
    Math(String),
}

fn date(op: &Operand) -> Result<DateValue> {
    let bad = || MqlError {
        message: "Use a date like 2026-11-01 or \"2026-11-01 14:30\", a relative time like -7d or -2w, \
                  or now(), startOfDay(), startOfWeek(), startOfMonth() or startOfYear()."
            .to_owned(),
        start: op.start,
        end: op.end,
    };
    match &op.value {
        Val::Function(f) => Ok(DateValue::Math(
            match f.as_str() {
                "now" => "now",
                "startofday" => "now/d",
                "startofweek" => "now/w",
                "startofmonth" => "now/M",
                "startofyear" => "now/y",
                _ => return Err(bad()),
            }
            .to_owned(),
        )),
        Val::Text(raw) => {
            let raw = raw.trim();
            // Relative: [+-]N(m|h|d|w); no sign means from now on.
            if let Some(unit) = raw.chars().last().filter(|u| matches!(u, 'm' | 'h' | 'd' | 'w')) {
                let body = &raw[..raw.len() - 1];
                let (sign, digits) = match body.strip_prefix('-') {
                    Some(d) => ('-', d),
                    None => ('+', body.strip_prefix('+').unwrap_or(body)),
                };
                if !digits.is_empty() && digits.len() <= 5 && digits.chars().all(|c| c.is_ascii_digit()) {
                    let n: u32 = digits.parse().map_err(|_| bad())?;
                    return Ok(DateValue::Math(format!("now{sign}{n}{unit}")));
                }
                return Err(bad());
            }
            let day = time::macros::format_description!("[year]-[month]-[day]");
            let minute = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
            if let Ok(d) = time::Date::parse(raw, &day) {
                return Ok(DateValue::Day(d.format(&day).map_err(|_| bad())?));
            }
            if let Ok(t) = time::PrimitiveDateTime::parse(raw, &minute) {
                let iso = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:00Z");
                return Ok(DateValue::Minute(t.format(&iso).map_err(|_| bad())?));
            }
            Err(bad())
        }
    }
}

fn not(query: Value) -> Value {
    json!({ "bool": { "must_not": [query] } })
}

struct Compiler<'a> {
    ctx: &'a Context<'a>,
}

impl Compiler<'_> {
    fn expr(&self, expr: &Expr) -> Result<Value> {
        Ok(match expr {
            Expr::And(parts) => json!({ "bool": { "must": parts.iter().map(|p| self.expr(p)).collect::<Result<Vec<_>>>()? } }),
            Expr::Or(parts) => json!({ "bool": {
                "should": parts.iter().map(|p| self.expr(p)).collect::<Result<Vec<_>>>()?,
                "minimum_should_match": 1
            } }),
            Expr::Not(inner) => not(self.expr(inner)?),
            Expr::Clause(c) => self.clause(c)?,
        })
    }

    fn person(&self, op: &Operand, field: Field) -> Result<Uuid> {
        match &op.value {
            Val::Function(f) if f == "currentuser" => Ok(self.ctx.current_user),
            Val::Function(f) => fail(format!("{f}() cannot be used here; try currentUser()."), op.start, op.end),
            Val::Text(t) => {
                if let Ok(id) = Uuid::parse_str(t.trim()) {
                    return Ok(id);
                }
                let Some(people) = self.ctx.people else {
                    return fail(format!("Guests can only search {} by currentUser().", field.name()), op.start, op.end);
                };
                match people.get(&person_key(t)).map(Vec::as_slice) {
                    Some([one]) => Ok(*one),
                    Some([]) | None => fail(
                        format!("No one in this workspace has the email address or name \"{}\".", t.trim()),
                        op.start,
                        op.end,
                    ),
                    Some(many) => fail(
                        format!("{} people are called \"{}\". Use their email address instead.", many.len(), t.trim()),
                        op.start,
                        op.end,
                    ),
                }
            }
        }
    }

    /// The term values a clause compares against.
    fn terms(&self, c: &Clause) -> Result<(&'static str, Vec<Value>)> {
        let mut field = match c.field {
            Field::Project => "project_key",
            Field::Key => "key",
            Field::Type => "type",
            Field::Status => "status",
            Field::StatusCategory => "status_category",
            Field::Priority => "priority",
            Field::Assignee => "assignee_id",
            Field::Reporter => "reporter_id",
            Field::Labels => "labels",
            _ => unreachable!("not a term field"),
        };
        let mut values = Vec::new();
        for op in &c.values {
            values.push(match c.field {
                Field::Project => {
                    let t = text(op)?.trim();
                    match Uuid::parse_str(t) {
                        Ok(id) => {
                            field = "project_id";
                            json!(id)
                        }
                        Err(_) => json!(t.to_uppercase()),
                    }
                }
                Field::Key => json!(text(op)?.trim()),
                Field::Labels => json!(text(op)?),
                Field::StatusCategory => json!(status_category(op)?),
                Field::Assignee | Field::Reporter => json!(self.person(op, c.field)?),
                _ => json!(slug(text(op)?)),
            });
        }
        // Mixing project ids and keys in one list would compare keys against ids.
        if c.field == Field::Project
            && field == "project_id"
            && c.values.iter().any(|v| matches!(&v.value, Val::Text(t) if Uuid::parse_str(t.trim()).is_err()))
        {
            return fail("Use either project keys or project ids in one list, not both.", c.start, c.end);
        }
        Ok((field, values))
    }

    fn clause(&self, c: &Clause) -> Result<Value> {
        match c.field {
            Field::Summary | Field::Description | Field::Comment | Field::Text => self.text_clause(c),
            Field::Created | Field::Updated | Field::Resolved => self.date_clause(c),
            Field::Priority if matches!(c.op, Op::Lt | Op::Le | Op::Gt | Op::Ge) => {
                let op = &c.values[0];
                let Some(rank) = priority_rank(&slug(text(op)?)) else {
                    return fail("Compare priorities by name: Lowest, Low, Medium, High or Highest.", op.start, op.end);
                };
                Ok(json!({ "range": { "priority_rank": { range_key(c.op): rank } } }))
            }
            _ => {
                let exists = |field: &str| json!({ "exists": { "field": field } });
                let field = match c.field {
                    Field::Assignee => "assignee_id",
                    Field::Reporter => "reporter_id",
                    _ => "labels",
                };
                match c.op {
                    Op::IsEmpty => Ok(not(exists(field))),
                    Op::IsNotEmpty => Ok(exists(field)),
                    _ => {
                        let (field, values) = self.terms(c)?;
                        let positive = json!({ "terms": { field: values } });
                        Ok(if matches!(c.op, Op::Ne | Op::NotIn) { not(positive) } else { positive })
                    }
                }
            }
        }
    }

    fn text_clause(&self, c: &Clause) -> Result<Value> {
        let op = &c.values[0];
        let words = text(op)?.trim();
        if words.trim_matches('*').trim().is_empty() {
            return fail("Search for at least one word.", op.start, op.end);
        }
        let fields: Vec<&str> = match c.field {
            Field::Summary => vec!["summary"],
            Field::Description => vec!["description"],
            Field::Comment => vec!["comments"],
            _ => vec!["summary", "description", "comments"],
        };
        // A trailing * matches words starting with the last word typed.
        let query = match words.strip_suffix('*') {
            Some(prefix) => json!({ "multi_match": { "query": prefix.trim(), "type": "phrase_prefix", "fields": fields } }),
            None => json!({ "multi_match": { "query": words, "type": "cross_fields", "operator": "and", "fields": fields } }),
        };
        Ok(if c.op == Op::NotContains { not(query) } else { query })
    }

    fn date_clause(&self, c: &Clause) -> Result<Value> {
        let field = match c.field {
            Field::Created => "created_at",
            Field::Updated => "updated_at",
            _ => "resolved_at",
        };
        match c.op {
            Op::IsEmpty => return Ok(not(json!({ "exists": { "field": field } }))),
            Op::IsNotEmpty => return Ok(json!({ "exists": { "field": field } })),
            _ => {}
        }
        let op = &c.values[0];
        let value = date(op)?;
        if c.op == Op::Eq {
            let (from, to) = match value {
                DateValue::Day(d) => (d.clone(), format!("{d}||+1d")),
                DateValue::Minute(m) => (m.clone(), format!("{m}||+1m")),
                DateValue::Math(_) => {
                    return fail("Use >= and < to compare with now() or a relative time.", op.start, op.end);
                }
            };
            return Ok(json!({ "range": { field: { "gte": from, "lt": to } } }));
        }
        let bound = match value {
            DateValue::Day(d) => d,
            DateValue::Minute(m) => m,
            DateValue::Math(m) => m,
        };
        Ok(json!({ "range": { field: { range_key(c.op): bound } } }))
    }
}

fn range_key(op: Op) -> &'static str {
    match op {
        Op::Lt => "lt",
        Op::Le => "lte",
        Op::Gt => "gt",
        _ => "gte",
    }
}

fn has_text(expr: &Expr) -> bool {
    match expr {
        Expr::And(parts) | Expr::Or(parts) => parts.iter().any(has_text),
        Expr::Not(_) => false,
        Expr::Clause(c) => matches!(c.field, Field::Summary | Field::Description | Field::Comment | Field::Text),
    }
}

pub fn compile(query: &Query, ctx: &Context<'_>) -> Result<Compiled> {
    let compiled = match &query.filter {
        Some(expr) => Compiler { ctx }.expr(expr)?,
        None => json!({ "match_all": {} }),
    };
    let mut sort = Vec::new();
    let mut sort_values = Vec::new();
    let dir = |d: bool| if d { "desc" } else { "asc" };
    for (field, descending) in &query.order {
        match field {
            SortField::Created | SortField::Updated | SortField::Resolved => {
                let name = match field {
                    SortField::Created => "created_at",
                    SortField::Updated => "updated_at",
                    _ => "resolved_at",
                };
                sort.push(json!({ name: { "order": dir(*descending), "missing": "_last" } }));
                sort_values.push(SortValue::Number);
            }
            SortField::Priority => {
                sort.push(
                    json!({ "priority_rank": { "order": dir(*descending), "missing": "_last", "unmapped_type": "integer" } }),
                );
                sort_values.push(SortValue::Number);
            }
            SortField::Key => {
                sort.push(json!({ "project_key": { "order": dir(*descending) } }));
                sort.push(json!({ "key_number": { "order": dir(*descending), "missing": "_last", "unmapped_type": "long" } }));
                sort_values.extend([SortValue::Text, SortValue::Number]);
            }
        }
    }
    if query.order.is_empty() {
        if query.filter.as_ref().is_some_and(has_text) {
            sort.push(json!({ "_score": "desc" }));
            sort_values.push(SortValue::Number);
        }
        sort.push(json!({ "updated_at": "desc" }));
        sort_values.push(SortValue::Number);
    }
    sort.push(json!({ "issue_id": "asc" }));
    sort_values.push(SortValue::Text);
    Ok(Compiled { query: compiled, sort: Value::Array(sort), sort_values })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Uuid {
        Uuid::from_u128(7)
    }

    fn compile_str(q: &str) -> Result<Compiled> {
        let mut people = HashMap::new();
        people.insert("maya@example.com".to_owned(), vec![Uuid::from_u128(1)]);
        people.insert("sam lee".to_owned(), vec![Uuid::from_u128(2), Uuid::from_u128(3)]);
        compile(&parse(q)?, &Context { current_user: me(), people: Some(&people) })
    }

    fn error(q: &str) -> (String, usize, usize) {
        let e = compile_str(q).unwrap_err();
        (e.message, e.start, e.end)
    }

    #[test]
    fn parses_precedence_and_lists() {
        let q = parse("project = pay AND (status = \"In Review\" OR priority in (High, highest)) order by updated desc, key")
            .unwrap();
        assert_eq!(q.order, vec![(SortField::Updated, true), (SortField::Key, false)]);
        let Some(Expr::And(parts)) = &q.filter else { panic!("{q:?}") };
        assert!(matches!(&parts[1], Expr::Or(o) if o.len() == 2));

        // OR binds looser than AND.
        let q = parse("a1 = x").unwrap_err();
        assert_eq!((q.start, q.end), (0, 2));
        let Some(Expr::Or(parts)) = parse("type = bug AND status = todo OR type = task").unwrap().filter else { panic!() };
        assert!(matches!(&parts[0], Expr::And(a) if a.len() == 2));
        assert!(parse("").unwrap().filter.is_none());
        assert!(parse("ORDER BY created").unwrap().filter.is_none());
    }

    #[test]
    fn compiles_to_term_and_range_queries() {
        let c = compile_str("project = pay AND status = \"In Review\" AND assignee = currentUser()").unwrap();
        let must = &c.query["bool"]["must"];
        assert_eq!(must[0], json!({ "terms": { "project_key": ["PAY"] } }));
        assert_eq!(must[1], json!({ "terms": { "status": ["in_review"] } }));
        assert_eq!(must[2], json!({ "terms": { "assignee_id": [me()] } }));

        let c = compile_str("assignee != maya@example.com").unwrap();
        assert_eq!(c.query, json!({ "bool": { "must_not": [{ "terms": { "assignee_id": [Uuid::from_u128(1)] } }] } }));
        let c = compile_str("assignee is empty").unwrap();
        assert_eq!(c.query, json!({ "bool": { "must_not": [{ "exists": { "field": "assignee_id" } }] } }));
        assert_eq!(compile_str("assignee = EMPTY").unwrap().query, c.query);

        assert_eq!(compile_str("priority >= high").unwrap().query, json!({ "range": { "priority_rank": { "gte": 4 } } }));
        assert_eq!(
            compile_str("statusCategory = \"in progress\"").unwrap().query,
            json!({ "terms": { "status_category": ["in_progress"] } })
        );
    }

    #[test]
    fn dates_become_validated_date_math() {
        let q = |s: &str| compile_str(s).unwrap().query;
        assert_eq!(q("created >= -7d"), json!({ "range": { "created_at": { "gte": "now-7d" } } }));
        assert_eq!(q("updated < startOfWeek()"), json!({ "range": { "updated_at": { "lt": "now/w" } } }));
        assert_eq!(
            q("created = 2026-11-01"),
            json!({ "range": { "created_at": { "gte": "2026-11-01", "lt": "2026-11-01||+1d" } } })
        );
        assert_eq!(q("resolved > \"2026-11-01 14:30\""), json!({ "range": { "resolved_at": { "gt": "2026-11-01T14:30:00Z" } } }));
        assert_eq!(q("resolved is not empty"), json!({ "exists": { "field": "resolved_at" } }));
        for bad in ["created > yesterday", "created > \"now-1d||+1d\"", "created > -7y", "created > 2026-13-01", "created = -1d"]
        {
            assert!(compile_str(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn text_is_matched_never_parsed() {
        let c = compile_str("text ~ \"refund* OR status:done\"").unwrap();
        assert_eq!(c.query["multi_match"]["query"], "refund* OR status:done");
        assert_eq!(c.query["multi_match"]["operator"], "and");
        let c = compile_str("summary ~ refun*").unwrap();
        assert_eq!(c.query["multi_match"]["type"], "phrase_prefix");
        assert_eq!(c.query["multi_match"]["query"], "refun");
        assert_eq!(c.sort[0], json!({ "_score": "desc" }), "text queries rank by relevance by default");
        let c = compile_str("comment !~ flaky").unwrap();
        assert!(c.query["bool"]["must_not"][0]["multi_match"].is_object());
    }

    #[test]
    fn sorting_and_cursor_shapes() {
        let c = compile_str("ORDER BY priority DESC, key").unwrap();
        assert_eq!(c.sort_values, [SortValue::Number, SortValue::Text, SortValue::Number, SortValue::Text]);
        assert_eq!(c.sort.as_array().unwrap().last().unwrap(), &json!({ "issue_id": "asc" }));
        assert_eq!(compile_str("").unwrap().sort, json!([{ "updated_at": "desc" }, { "issue_id": "asc" }]));
    }

    #[test]
    fn errors_point_at_the_problem() {
        assert_eq!(error("status = done AND"), ("Expected a field here, such as project, status or assignee.".into(), 17, 17));
        assert_eq!(error("stauts = done").1..error("stauts = done").2, 0..6);
        assert!(error("stauts = done").0.contains("Unknown field \"stauts\""));
        assert_eq!(error("status ~ done").0, "status supports =, !=, IN and NOT IN.");
        assert_eq!(error("status = done status = todo").0, "Expected AND, OR or ORDER BY here.");
        assert_eq!(error("summary ~ \"open").0, "This text is missing its closing quote.");
        assert_eq!(error("assignee = \"sam lee\"").0, "2 people are called \"sam lee\". Use their email address instead.");
        assert!(error("assignee = nobody@example.com").0.starts_with("No one in this workspace"));
        assert_eq!(error("project in (PAY, ").0, "Expected a value here.");
        assert_eq!(error("project in (PAY").0, "Expected a comma or a closing parenthesis.");
        assert_eq!(error("ORDER BY summary").0, "Sort by created, updated, resolved, priority or key.");
        assert_eq!(error("assignee = now()").0, "now() cannot be used here; try currentUser().");
        assert_eq!(error("status = and").0, "Expected a value here. Quote it if it is a word like \"and\".");
        let (_, s, e) = error("type = bug AND (status = done");
        assert_eq!((s, e), (29, 29));
    }

    #[test]
    fn limits_hold() {
        let deep = format!("{}status = done{}", "(".repeat(40), ")".repeat(40));
        assert_eq!(error(&deep).0, "The query is nested too deeply.");
        let deep_not = format!("{}status = done", "NOT ".repeat(40));
        assert_eq!(error(&deep_not).0, "The query is nested too deeply.");
        let many = vec!["type = bug"; 101].join(" OR ");
        assert_eq!(error(&many).0, "The query has too many conditions.");
        let list = format!("labels in ({})", vec!["a"; 101].join(","));
        assert_eq!(error(&list).0, "A list can hold at most 100 values.");
        assert!(parse(&"a".repeat(2001)).is_err());
        assert!(parse("summary ~ \"a\u{0}b\"").is_err());
    }

    #[test]
    fn guests_cannot_look_people_up() {
        let q = parse("assignee = maya@example.com").unwrap();
        let e = compile(&q, &Context { current_user: me(), people: None }).unwrap_err();
        assert_eq!(e.message, "Guests can only search assignee by currentUser().");
        assert_eq!(
            person_terms(&parse("assignee in (Maya@Example.com, currentUser()) OR reporter = ' Sam Lee '").unwrap()),
            ["maya@example.com", "sam lee"]
        );
    }
}
