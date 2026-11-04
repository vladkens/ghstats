// a query filter modeled on GitHub search, e.g. `(stars:>=25 OR age:<3m) !fork:true`; knows
// nothing about ghstats, the host describes its fields and supplies their values

use std::fmt;
use std::ops::{Bound, RangeBounds};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
  Int,
  Duration, // in days
  Bool,
  Glob,
}

#[derive(Debug, Clone, Copy)]
pub struct Schema<'a> {
  pub fields: &'a [(&'a str, Kind)],
  pub default: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
  Num(i64),
  Bool(bool),
  Str(&'a str),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorKind {
  UnknownField(String),
  BadValue(String, String),
  Unclosed,
  Unmatched,
  Dangling(String),
  EmptyGroup,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
  pub kind: ErrorKind,
  pub pos: usize,
}

impl fmt::Display for Error {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    match &self.kind {
      ErrorKind::UnknownField(x) => write!(f, "unknown field `{x}`"),
      ErrorKind::BadValue(field, x) => write!(f, "bad value `{x}` for `{field}`"),
      ErrorKind::Unclosed => write!(f, "unclosed `(`"),
      ErrorKind::Unmatched => write!(f, "unmatched `)`"),
      ErrorKind::Dangling(op) => write!(f, "nothing after `{op}`"),
      ErrorKind::EmptyGroup => write!(f, "empty group"),
    }?;

    write!(f, " at {}", self.pos)
  }
}

impl std::error::Error for Error {}

fn err<T>(kind: ErrorKind, pos: usize) -> Result<T, Error> {
  Err(Error { kind, pos })
}

#[derive(Debug, Clone)]
enum Test {
  Range(Bound<i64>, Bound<i64>),
  Bool(bool),
  Glob(String),
}

#[derive(Debug, Clone)]
enum Node {
  All(Vec<Node>),
  Any(Vec<Node>),
  Not(Box<Node>),
  Term(String, Test),
}

impl Node {
  fn eval<'a>(&self, get: &impl Fn(&str) -> Option<Value<'a>>) -> bool {
    match self {
      Node::All(xs) => xs.iter().all(|x| x.eval(get)),
      Node::Any(xs) => xs.iter().any(|x| x.eval(get)),
      Node::Not(x) => !x.eval(get),
      Node::Term(field, test) => match (test, get(field)) {
        (Test::Range(lo, hi), Some(Value::Num(x))) => (*lo, *hi).contains(&x),
        (Test::Bool(want), Some(Value::Bool(x))) => x == *want,
        (Test::Glob(pat), Some(Value::Str(x))) => glob(pat, &x.to_lowercase()),
        _ => false,
      },
    }
  }
}

#[derive(Debug, Clone)]
pub struct Filter(Node);

impl Filter {
  pub fn parse(input: &str, schema: &Schema) -> Result<Self, Error> {
    let mut parser = Parser { toks: lex(input), at: 0, schema };
    if parser.toks.is_empty() {
      return Ok(Self(Node::All(vec![])));
    }

    let node = parser.or()?;
    match parser.peek() {
      Some((pos, _)) => err(ErrorKind::Unmatched, pos),
      None => Ok(Self(node)),
    }
  }

  // `get` returns a field's value, `None` when unknown, which makes its terms false
  pub fn matches<'a>(&self, get: impl Fn(&str) -> Option<Value<'a>>) -> bool {
    self.0.eval(&get)
  }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok<'a> {
  Open,
  Close,
  Not,
  And(&'a str),
  Or,
  Word(&'a str),
}

fn lex(input: &str) -> Vec<(usize, Tok<'_>)> {
  let mut toks = Vec::new();
  let mut pos = 0;
  while let Some(c) = input[pos..].chars().next() {
    if c.is_whitespace() {
      pos += c.len_utf8();
      continue;
    }

    let len = match c {
      '(' | ')' | '!' | ',' => 1,
      _ => input[pos..]
        .find(|c: char| c.is_whitespace() || "(),".contains(c))
        .unwrap_or(input.len() - pos),
    };

    let tok = match &input[pos..pos + len] {
      "(" => Tok::Open,
      ")" => Tok::Close,
      "!" => Tok::Not,
      "," => Tok::And(","),
      "AND" => Tok::And("AND"),
      "OR" => Tok::Or,
      x => Tok::Word(x),
    };

    toks.push((pos, tok));
    pos += len;
  }

  toks
}

struct Parser<'a, 's> {
  toks: Vec<(usize, Tok<'a>)>,
  at: usize,
  schema: &'s Schema<'s>,
}

impl<'a> Parser<'a, '_> {
  fn peek(&self) -> Option<(usize, Tok<'a>)> {
    self.toks.get(self.at).copied()
  }

  // an operator must be followed by a term, a group or `!`
  fn operand(&self, pos: usize, op: &str) -> Result<(), Error> {
    match self.peek() {
      Some((_, Tok::Word(_) | Tok::Open | Tok::Not)) => Ok(()),
      _ => err(ErrorKind::Dangling(op.into()), pos),
    }
  }

  fn or(&mut self) -> Result<Node, Error> {
    let mut xs = vec![self.and()?];
    while let Some((pos, Tok::Or)) = self.peek() {
      self.at += 1;
      self.operand(pos, "OR")?;
      xs.push(self.and()?);
    }

    Ok(if xs.len() == 1 { xs.remove(0) } else { Node::Any(xs) })
  }

  fn and(&mut self) -> Result<Node, Error> {
    let mut xs = vec![self.unary()?];
    loop {
      match self.peek() {
        Some((pos, Tok::And(op))) => {
          self.at += 1;
          self.operand(pos, op)?;
        }
        Some((_, Tok::Word(_) | Tok::Open | Tok::Not)) => {}
        _ => break,
      }

      xs.push(self.unary()?);
    }

    Ok(if xs.len() == 1 { xs.remove(0) } else { Node::All(xs) })
  }

  fn unary(&mut self) -> Result<Node, Error> {
    // every caller checks for a token first: `parse`, `(` and the operators
    let (pos, tok) = self.toks[self.at];
    self.at += 1;
    match tok {
      Tok::Not => {
        self.operand(pos, "!")?;
        Ok(Node::Not(Box::new(self.unary()?)))
      }
      Tok::Open => {
        match self.peek() {
          Some((_, Tok::Close)) => return err(ErrorKind::EmptyGroup, pos),
          None => return err(ErrorKind::Unclosed, pos),
          _ => {}
        }

        let node = self.or()?;
        match self.peek() {
          Some((_, Tok::Close)) => self.at += 1,
          _ => return err(ErrorKind::Unclosed, pos),
        }

        Ok(node)
      }
      Tok::Word(x) => self.term(pos, x),
      Tok::Close => err(ErrorKind::Unmatched, pos),
      Tok::Or => err(ErrorKind::Dangling("OR".into()), pos),
      Tok::And(op) => err(ErrorKind::Dangling(op.into()), pos),
    }
  }

  fn term(&self, pos: usize, word: &str) -> Result<Node, Error> {
    let (field, value, at) = match word.split_once(':') {
      Some((field, value)) => (field.to_lowercase(), value, pos + field.len() + 1),
      None => (self.schema.default.to_string(), word, pos),
    };

    let Some(&(_, kind)) = self.schema.fields.iter().find(|x| x.0 == field) else {
      return err(ErrorKind::UnknownField(field), pos);
    };

    let lower = value.to_lowercase();
    let test = match kind {
      Kind::Int => range(&lower, int),
      Kind::Duration => range(&lower, days),
      Kind::Bool => match lower.as_str() {
        "true" => Some(Test::Bool(true)),
        "false" => Some(Test::Bool(false)),
        _ => None,
      },
      Kind::Glob => (!lower.is_empty()).then_some(Test::Glob(lower)),
    };

    match test {
      Some(test) => Ok(Node::Term(field, test)),
      None => err(ErrorKind::BadValue(field, value.into()), at),
    }
  }
}

// `25`, `>25`, `>=25`, `<25`, `<=25`, `10..50`, `10..*`, `*..50`
fn range(value: &str, num: fn(&str) -> Option<i64>) -> Option<Test> {
  let bound = |x: &str| match x {
    "*" => Some(Bound::Unbounded),
    x => num(x).map(Bound::Included),
  };

  let (lo, hi) = if let Some((lo, hi)) = value.split_once("..") {
    let (lo, hi) = (bound(lo)?, bound(hi)?);

    // a reversed range like `100..10` matches nothing, so it's surely a typo
    if let (Bound::Included(lo), Bound::Included(hi)) = (lo, hi)
      && lo > hi
    {
      return None;
    }

    (lo, hi)
  } else if let Some(x) = value.strip_prefix(">=") {
    (Bound::Included(num(x)?), Bound::Unbounded)
  } else if let Some(x) = value.strip_prefix("<=") {
    (Bound::Unbounded, Bound::Included(num(x)?))
  } else if let Some(x) = value.strip_prefix('>') {
    (Bound::Excluded(num(x)?), Bound::Unbounded)
  } else if let Some(x) = value.strip_prefix('<') {
    (Bound::Unbounded, Bound::Excluded(num(x)?))
  } else {
    let x = num(value)?;
    (Bound::Included(x), Bound::Included(x))
  };

  Some(Test::Range(lo, hi))
}

// a plain decimal times `unit`, so `1.5` with `k` is 1500; the result must be whole
fn scaled(x: &str, unit: i64) -> Option<i64> {
  let (int, frac) = x.split_once('.').unwrap_or((x, ""));
  if int.is_empty() || !int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit()) {
    return None;
  }

  let base = 10i64.checked_pow(frac.len() as u32)?;
  let x = format!("{int}{frac}").parse::<i64>().ok()?.checked_mul(unit)?;
  (x % base == 0).then_some(x / base)
}

fn int(x: &str) -> Option<i64> {
  match x.strip_suffix('k') {
    Some(x) => scaled(x, 1000),
    None => scaled(x, 1),
  }
}

fn days(x: &str) -> Option<i64> {
  let unit = match x.chars().last()? {
    'd' => 1,
    'w' => 7,
    'm' => 30,
    'y' => 365,
    _ => return None,
  };

  scaled(&x[..x.len() - 1], unit)
}

// `*` matches any run of chars, everything else matches itself
fn glob(pat: &str, s: &str) -> bool {
  let parts: Vec<&str> = pat.split('*').collect();
  if parts.len() == 1 {
    return pat == s;
  }

  let (first, last) = (parts[0], parts[parts.len() - 1]);
  if s.len() < first.len() + last.len() || !s.starts_with(first) || !s.ends_with(last) {
    return false;
  }

  let mut rest = &s[first.len()..s.len() - last.len()];
  for part in &parts[1..parts.len() - 1] {
    match rest.find(part) {
      Some(i) => rest = &rest[i + part.len()..],
      None => return false,
    }
  }

  true
}

#[cfg(test)]
mod tests {
  use super::*;

  const SCHEMA: Schema = Schema {
    fields: &[
      ("n", Kind::Int),
      ("age", Kind::Duration),
      ("fork", Kind::Bool),
      ("name", Kind::Glob),
    ],
    default: "name",
  };

  // (name, n, age in days, fork)
  type Rec = (&'static str, i64, Option<i64>, bool);

  fn check(cases: &[(&str, Rec, bool)]) {
    for (input, rec, want) in cases {
      let filter = Filter::parse(input, &SCHEMA).unwrap_or_else(|e| panic!("{input}: {e}"));
      let got = filter.matches(|field| match field {
        "name" => Some(Value::Str(rec.0)),
        "n" => Some(Value::Num(rec.1)),
        "age" => rec.2.map(Value::Num),
        "fork" => Some(Value::Bool(rec.3)),
        _ => None,
      });

      assert_eq!(got, *want, "{input} {rec:?}");
    }
  }

  const R: Rec = ("o/r", 25, Some(10), false);

  #[test]
  fn precedence() {
    check(&[
      ("", R, true),
      ("  ", R, true),
      ("n:1 OR n:25 n:2", R, false), // n:1 OR (n:25 AND n:2)
      ("n:25 OR n:1 n:2", R, true),
      ("!n:1 n:25", R, true), // (!n:1) AND n:25
      ("!n:25 OR n:25", R, true),
      ("n:25 AND n:1 OR n:25", R, true),
      ("n:25, n:1", R, false), // comma = AND
      ("n:25,fork:false", R, true),
    ]);
  }

  #[test]
  fn groups() {
    check(&[
      ("(n:1 OR n:25) n:2", R, false),
      ("(n:1 OR n:25) fork:false", R, true),
      ("!(n:1 OR n:2)", R, true),
      ("!(n:1 OR n:25)", R, false),
      ("((n:25))", R, true),
      ("!!n:25", R, true),
    ]);
  }

  #[test]
  fn ranges() {
    check(&[
      ("n:>24", R, true),
      ("n:>25", R, false),
      ("n:>=25", R, true),
      ("n:<25", R, false),
      ("n:<=25", R, true),
      ("n:10..25", R, true),
      ("n:26..50", R, false),
      ("n:25..*", R, true),
      ("n:*..24", R, false),
      ("n:*..*", R, true),
      ("n:1.1k", ("o/r", 1100, None, false), true),
      ("n:<1.5k", R, true),
      ("n:>=1k", ("o/r", 1000, None, false), true),
    ]);
  }

  #[test]
  fn durations() {
    check(&[
      ("age:10d", R, true),
      ("age:<2w", R, true),
      ("age:>1w", R, true),
      ("age:<3m", ("o/r", 0, Some(89), false), true),
      ("age:<3m", ("o/r", 0, Some(90), false), false),
      ("age:>=1y", ("o/r", 0, Some(365), false), true),
      ("age:1w..2w", R, true),
      ("age:<1y", ("o/r", 0, None, false), false), // missing value
      ("!age:<1y", ("o/r", 0, None, false), true),
    ]);
  }

  #[test]
  fn bools_and_globs() {
    check(&[
      ("fork:false", R, true),
      ("fork:TRUE", ("o/r", 0, None, true), true),
      ("name:o/r", R, true),
      ("name:O/*", R, true),
      ("o/*", R, true), // bare word
      ("*/r", R, true),
      ("o*r", R, true),
      ("o/r*", R, true),
      ("o/rr", R, false),
      ("x/*", R, false),
      ("*/*/*", R, false),
      ("a*b*c", ("abc", 0, None, false), true),
      ("a*bc*c", ("abc", 0, None, false), false),
      ("or", R, false), // lowercase `or` is a bare word
    ]);
  }

  #[test]
  fn errors() {
    use ErrorKind::*;

    let bad = |f: &str, x: &str| BadValue(f.into(), x.into());
    let cases = [
      ("starz:>1", UnknownField("starz".into()), 0),
      ("n:1 x:1", UnknownField("x".into()), 4),
      ("n:abc", bad("n", "abc"), 2),
      ("n:1.5", bad("n", "1.5"), 2),
      ("n:1.2345k", bad("n", "1.2345k"), 2),
      ("n:>", bad("n", ">"), 2),
      ("n:-1", bad("n", "-1"), 2),
      ("n:1..x", bad("n", "1..x"), 2),
      ("n:100..10", bad("n", "100..10"), 2),
      ("age:3", bad("age", "3"), 4),
      ("age:3h", bad("age", "3h"), 4),
      ("fork:yes", bad("fork", "yes"), 5),
      ("name:", bad("name", ""), 5),
      ("(n:1", Unclosed, 0),
      ("(", Unclosed, 0),
      ("n:1 (n:2 OR (n:3)", Unclosed, 4),
      ("n:1)", Unmatched, 3),
      (")", Unmatched, 0),
      ("()", EmptyGroup, 0),
      ("n:1 ( )", EmptyGroup, 4),
      ("n:1 OR", Dangling("OR".into()), 4),
      ("OR n:1", Dangling("OR".into()), 0),
      ("n:1 OR OR n:2", Dangling("OR".into()), 4),
      ("n:1 AND", Dangling("AND".into()), 4),
      ("n:1,", Dangling(",".into()), 3),
      (",n:1", Dangling(",".into()), 0),
      ("n:1,,n:2", Dangling(",".into()), 3),
      ("n:1 !", Dangling("!".into()), 4),
      ("(n:1 OR)", Dangling("OR".into()), 5),
    ];

    for (input, kind, pos) in cases {
      assert_eq!(Filter::parse(input, &SCHEMA).unwrap_err(), Error { kind, pos }, "{input}");
    }

    let e = Filter::parse("starz:>1", &SCHEMA).unwrap_err();
    assert_eq!(e.to_string(), "unknown field `starz` at 0");
  }
}
