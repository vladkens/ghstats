// GHS_FILTER and GHS_SHOW queries modeled on GitHub search, e.g. `(stars:>=25 OR age:<3m) !fork`

use std::ops::RangeInclusive;

// GHS_FILTER takes only the first three: a repo it drops stops syncing, so its counts would freeze
const FIELDS: [&str; 8] = ["repo", "fork", "archived", "stars", "forks", "issues", "prs", "age"];

#[derive(Debug, Clone, Copy, Default)]
pub struct Repo<'a> {
  pub name: &'a str,
  pub fork: bool,
  pub archived: bool,
  pub stars: i64,
  pub forks: i64,
  pub issues: i64,
  pub prs: i64,
  pub age: Option<i64>, // days, unknown until a sync
}

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
  pub pos: usize,
  pub msg: String,
}

fn err<T>(pos: usize, msg: impl Into<String>) -> Result<T, Error> {
  Err(Error { pos, msg: msg.into() })
}

#[derive(Debug)]
pub enum Filter {
  All(Vec<Filter>),
  Any(Vec<Filter>),
  Not(Box<Filter>),
  Repo(String),
  Range(usize, RangeInclusive<i64>), // a FIELDS index, `fork` and `archived` are 0 or 1
}

impl Filter {
  // `show` allows GHS_SHOW's fields
  pub fn parse(input: &str, show: bool) -> Result<Self, Error> {
    // a comma is no operator, so `stars:>1,000` or a legacy `a/*,!fork` can't pass for a query
    if let Some(pos) = input.find(',') {
      return err(pos, "unexpected `,` (use a space for AND)");
    }

    let (mut toks, mut rest) = (vec![], input.trim_start());
    while !rest.is_empty() {
      let end = rest.find(|c: char| c.is_whitespace() || "()".contains(c)).unwrap_or(rest.len());
      let len = if rest.starts_with(['(', ')', '!']) { 1 } else { end };
      toks.push((input.len() - rest.len(), &rest[..len]));
      rest = rest[len..].trim_start();
    }

    if toks.is_empty() {
      return Ok(Self::All(vec![]));
    }

    let mut parser = Parser { toks, at: 0, show };
    let filter = parser.list(true)?;
    match parser.peek() {
      Some((pos, _)) => err(pos, "unmatched `)`"),
      None => Ok(filter),
    }
  }

  pub fn matches(&self, x: &Repo) -> bool {
    match self {
      Self::All(xs) => xs.iter().all(|f| f.matches(x)),
      Self::Any(xs) => xs.iter().any(|f| f.matches(x)),
      Self::Not(f) => !f.matches(x),
      Self::Repo(pat) => glob(pat, &x.name.to_lowercase()),
      Self::Range(field, range) => {
        let (f, a) = (Some(x.fork.into()), Some(x.archived.into()));
        let values = [None, f, a, Some(x.stars), Some(x.forks), Some(x.issues), Some(x.prs), x.age];
        values[*field].is_some_and(|v| range.contains(&v))
      }
    }
  }
}

struct Parser<'a> {
  toks: Vec<(usize, &'a str)>,
  at: usize,
  show: bool,
}

impl<'a> Parser<'a> {
  fn peek(&self) -> Option<(usize, &'a str)> {
    self.toks.get(self.at).copied()
  }

  fn eat(&mut self, tok: &str) -> bool {
    let hit = self.peek().is_some_and(|x| x.1 == tok);
    self.at += usize::from(hit);
    hit
  }

  // a term, a group or `!` comes next
  fn operand(&self) -> bool {
    self.peek().is_some_and(|(_, x)| !matches!(x, ")" | "AND" | "OR"))
  }

  // `OR` of `AND` lists, where a space is `AND` too
  fn list(&mut self, or: bool) -> Result<Filter, Error> {
    let (op, mut xs) = (if or { "OR" } else { "AND" }, vec![]);
    loop {
      xs.push(if or { self.list(false)? } else { self.unary()? });
      let (pos, ate) = (self.peek().map_or(0, |x| x.0), self.eat(op));
      if ate && !self.operand() {
        return err(pos, format!("nothing after `{op}`"));
      }

      if !ate && (or || !self.operand()) {
        return Ok(if or { Filter::Any(xs) } else { Filter::All(xs) });
      }
    }
  }

  fn unary(&mut self) -> Result<Filter, Error> {
    // every caller checks for a token first: `parse`, `(` and the operators
    let (pos, tok) = self.toks[self.at];
    self.at += 1;
    match tok {
      "!" if !self.operand() => err(pos, "nothing after `!`"),
      "!" => Ok(Filter::Not(Box::new(self.unary()?))),
      "(" if self.peek().is_none() => err(pos, "unclosed `(`"),
      "(" if self.eat(")") => err(pos, "empty group"),
      "(" => {
        let filter = self.list(true)?;
        if self.eat(")") { Ok(filter) } else { err(pos, "unclosed `(`") }
      }
      ")" => err(pos, "unmatched `)`"),
      "AND" | "OR" => err(pos, format!("nothing after `{tok}`")),
      _ => self.term(pos, tok),
    }
  }

  fn term(&self, pos: usize, word: &str) -> Result<Filter, Error> {
    // a bare `fork` or `archived` is `fork:true`, any other bare word a repo
    let bare = word.to_lowercase();
    let (name, value, at) = match word.split_once(':') {
      Some((name, value)) => (name.to_lowercase(), value, pos + name.len() + 1),
      None if bare == "fork" || bare == "archived" => (bare, "true", pos),
      None => ("repo".into(), word, pos),
    };

    let fields = if self.show { &FIELDS[..] } else { &FIELDS[..3] };
    let Some(field) = fields.iter().position(|x| *x == name) else {
      return err(pos, format!("unknown field `{name}`"));
    };

    let lower = value.to_lowercase();
    let range = match field {
      0 if lower.matches('/').count() == 1 => return Ok(Filter::Repo(lower)),
      0 => return err(at, format!("expected owner/name, got `{value}`")),
      1 | 2 => ["false", "true"].iter().position(|x| *x == lower).map(|x| x as i64..=x as i64),
      _ => range(&lower, name == "age"),
    };

    let bad = || Error { pos: at, msg: format!("bad value `{value}` for `{name}`") };
    range.map(|range| Filter::Range(field, range)).ok_or_else(bad)
  }
}

// `25`, `>25`, `>=25`, `<25`, `<=25`, `10..50`, `10..*`, `*..50`
fn range(value: &str, age: bool) -> Option<RangeInclusive<i64>> {
  let (lo, hi) = match value.split_once("..") {
    Some((lo, hi)) => {
      let bound = |x: &str, star| if x == "*" { Some(star) } else { num(x, age) };
      (bound(lo, i64::MIN)?, bound(hi, i64::MAX)?)
    }
    None => {
      let op = [">=", "<=", ">", "<"].into_iter().find(|x| value.starts_with(x)).unwrap_or("");
      let x = num(&value[op.len()..], age)?;
      match op {
        ">=" => (x, i64::MAX),
        "<=" => (i64::MIN, x),
        ">" => (x.checked_add(1)?, i64::MAX),
        "<" => (i64::MIN, x - 1),
        _ => (x, x),
      }
    }
  };

  // a reversed range like `100..10` matches nothing, so it's surely a typo
  (lo <= hi).then_some(lo..=hi)
}

// a plain decimal times its unit, so `1.5k` is 1500 and `2w` 14 days; the result must be whole
fn num(x: &str, age: bool) -> Option<i64> {
  let units = if age { &[('d', 1), ('w', 7), ('m', 30), ('y', 365)][..] } else { &[('k', 1000)] };
  let (x, unit) = match units.iter().find(|u| x.ends_with(u.0)) {
    Some(&(_, unit)) => (&x[..x.len() - 1], unit),
    None if age => return None,
    None => (x, 1),
  };

  let (int, frac) = x.split_once('.').unwrap_or((x, ""));
  if int.is_empty()
    || x.ends_with('.')
    || !int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit())
  {
    return None;
  }

  let base = 10i64.checked_pow(frac.len() as u32)?;
  let x = format!("{int}{frac}").parse::<i64>().ok()?.checked_mul(unit)?;
  (x % base == 0).then_some(x / base)
}

// `*` matches any run of chars, everything else matches itself
fn glob(pat: &str, s: &str) -> bool {
  match pat.split_once('*') {
    Some((head, tail)) => {
      s.starts_with(head)
        && (head.len()..=s.len()).any(|i| s.is_char_boundary(i) && glob(tail, &s[i..]))
    }
    None => pat == s,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const R: Repo = Repo {
    name: "o/r",
    fork: false,
    archived: false,
    stars: 25,
    forks: 3,
    issues: 0,
    prs: 0,
    age: Some(10),
  };

  const FORK: Repo = Repo { fork: true, ..R };
  const ARCHIVED: Repo = Repo { archived: true, ..R };
  const GHSTATS: Repo = Repo { name: "vladkens/ghstats", ..R };
  const NO_AGE: Repo = Repo { age: None, ..R };

  #[test]
  fn matches() {
    let cases = [
      ("", R, true),
      ("stars:1 OR stars:25 stars:2", R, false), // stars:1 OR (stars:25 AND stars:2)
      ("stars:25 AND stars:1 OR stars:25", R, true),
      ("!stars:1 stars:25", R, true), // (!stars:1) AND stars:25
      ("(stars:1 OR stars:25) stars:2", R, false),
      ("!(stars:1 OR stars:25)", R, false),
      ("((stars:25)) !!forks:3", R, true),
      ("stars:>24 stars:>=25 stars:<=25 stars:10..25 stars:25..* stars:*..*", R, true),
      ("stars:>25 OR stars:<25 OR stars:26..50", R, false),
      ("stars:0.025k stars:<1.5k issues:0 prs:0", R, true),
      ("age:10d age:<2w age:>1w age:1w..2w age:<3m", R, true),
      ("age:>=1y", R, false),
      ("age:<1y", NO_AGE, false),
      ("!age:<1y", NO_AGE, true),
      ("fork:false archived:FALSE", R, true),
      ("fork", FORK, true),
      ("!FORK !Archived", R, true),
      ("!fork !archived", ARCHIVED, false),
      ("vladkens/* !fork", GHSTATS, true),
      ("*/ghstats", R, false),
      ("repo:O/* o/r* o*/*r", R, true),
      ("o/rr OR x/* OR o/r*r", R, false),
    ];

    for (input, rec, want) in cases {
      let filter = Filter::parse(input, true).unwrap_or_else(|e| panic!("{input}: {e:?}"));
      assert_eq!(filter.matches(&rec), want, "{input} {rec:?}");
    }
  }

  #[test]
  fn errors() {
    let cases = [
      ("stars:1 x:1", "unknown field `x`", 8),
      ("stars:abc", "bad value `abc` for `stars`", 6),
      ("stars:1.5", "bad value `1.5` for `stars`", 6),
      ("stars:1.", "bad value `1.` for `stars`", 6),
      ("stars:1.k", "bad value `1.k` for `stars`", 6),
      ("stars:>", "bad value `>` for `stars`", 6),
      ("stars:-1", "bad value `-1` for `stars`", 6),
      ("stars:100..10", "bad value `100..10` for `stars`", 6),
      ("age:3", "bad value `3` for `age`", 4),
      ("fork:yes", "bad value `yes` for `fork`", 5),
      ("-*", "expected owner/name, got `-*`", 0),
      ("fork:false repo:a/b/c", "expected owner/name, got `a/b/c`", 16),
      ("a/b (a/b OR (a/b)", "unclosed `(`", 4),
      ("a/b)", "unmatched `)`", 3),
      ("()", "empty group", 0),
      ("OR a/b", "nothing after `OR`", 0),
      ("a/b AND", "nothing after `AND`", 4),
      ("a/b !", "nothing after `!`", 4),
      ("(a/b OR)", "nothing after `OR`", 5),
      ("stars:>1,000", "unexpected `,` (use a space for AND)", 8),
    ];

    for (input, msg, pos) in cases {
      assert_eq!(
        Filter::parse(input, true).unwrap_err(),
        Error { pos, msg: msg.into() },
        "{input}"
      );
    }

    let e = Filter::parse("vladkens/* stars:>1", false).unwrap_err();
    assert_eq!(e, Error { pos: 11, msg: "unknown field `stars`".into() });
  }
}
