use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::db_client::{DbClient, RepoFilter, RepoSort, RepoTotals};
use crate::filter::{Filter, Repo};
use crate::gh_client::GhClient;
use crate::types::Res;

pub const DB_HEALTH_INTERVAL_SECS: u64 = 60 * 60;
pub const DB_NOT_WRITABLE_MESSAGE: &str = "SQLite database is not writable. ghstats Docker images run as a non-root user, so existing bind-mounted data directories may need an ownership fix. For docker run: docker run --rm -u root -v ./data:/app/data ghcr.io/vladkens/ghstats:latest chown -R appuser:appgroup /app/data. For Docker Compose: docker compose run --rm --user root ghstats chown -R appuser:appgroup /app/data";

// old comma rules like `*,!fork,owner/*`; a comma value that looks like a query goes to the parser
fn legacy_query(input: &str) -> Option<(String, Vec<String>)> {
  let query =
    input.contains([':', '(', ')']) || input.split_whitespace().any(|x| x == "OR" || x == "AND");
  let lone = ["*", "!fork", "!archived"].contains(&input.trim().to_lowercase().as_str());
  if query || !(lone || input.contains(',')) {
    return None;
  }

  let (mut all, mut includes, mut keep, mut warns) = (false, false, [None, None], vec![]);
  let (mut ex_exact, mut ex_wild, mut in_exact, mut in_wild) = (vec![], vec![], vec![], vec![]);
  for token in input.split(',').map(str::trim).filter(|x| !x.is_empty()) {
    let rule = token.to_lowercase();
    let name = rule.strip_prefix('!').unwrap_or(&rule);
    let ok =
      |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_alphanumeric() || "._-*".contains(c));
    includes |= name == rule && rule.matches('/').count() == 1;
    match rule.as_str() {
      "*" => all = true,
      "!fork" => keep[0] = Some("fork:false"),
      "!archived" => keep[1] = Some("archived:false"),
      _ if rule.matches('/').count() != 1 => warns.push(format!("ignored invalid rule `{token}`")),
      // the old matcher compared such rules literally, so they matched no GitHub name
      _ if !name.split('/').all(ok) => warns.push(format!("rule `{token}` matches no repo")),
      _ => {
        let list = match (name == rule, name.contains('*')) {
          (false, false) => &mut ex_exact,
          (false, true) => &mut ex_wild,
          (true, false) => &mut in_exact,
          (true, true) => &mut in_wild,
        };
        list.push(name.to_string());
      }
    }
  }

  // as the old matcher: `!EXe !(keep EXw) (INe OR keep (INw OR *))`, `*` implied without includes
  let all = all || !includes;
  let keep: Vec<String> = keep.iter().flatten().map(|x| x.to_string()).collect();
  let join =
    |xs: &[String], op| if xs.len() == 1 { xs[0].clone() } else { format!("({})", xs.join(op)) };

  let mut parts = vec![];
  if !all && in_wild.is_empty() {
    // every include rule was invalid
    if in_exact.is_empty() {
      return Some(("!*/*".into(), warns));
    }

    parts.push(join(&in_exact, " OR "));
  } else {
    // `*` or the wildcard includes, both under the flags
    let mut wide = keep.clone();
    if !all {
      wide.insert(0, join(&in_wild, " OR "));
    }

    if in_exact.is_empty() {
      parts = wide;
    } else if !wide.is_empty() {
      in_exact.push(join(&wide, " "));
      parts.push(join(&in_exact, " OR "));
    }
  }

  parts.extend(ex_exact.iter().map(|x| format!("!{x}")));
  if keep.is_empty() {
    parts.extend(ex_wild.iter().map(|x| format!("!{x}")));
  } else if !ex_wild.is_empty() {
    // the old matcher skipped wildcard rules for the flagged forks and archived
    parts.push(format!("!({} {})", keep.join(" "), join(&ex_wild, " OR ")));
  }

  Some((parts.join(" "), warns))
}

// an invalid value is logged and ignored, so an upgrade never stops a running instance
fn parse_or_all(name: &str, input: &str, show: bool) -> Filter {
  Filter::parse(input, show).unwrap_or_else(|e| {
    tracing::error!("invalid {name}, ignored: {} at {}", e.msg, e.pos);
    Filter::All(vec![])
  })
}

fn read_filter(input: &str) -> Filter {
  let Some((query, warns)) = legacy_query(input) else {
    tracing::info!("GHS_FILTER: {input:?}");
    return parse_or_all("GHS_FILTER", input, false);
  };

  warns.iter().for_each(|x| tracing::warn!("GHS_FILTER: {x}"));
  tracing::info!("GHS_FILTER: {input:?}, read as {query:?}");
  parse_or_all("GHS_FILTER", &query, false)
}

// a sort key of a numeric column shown on the home page, e.g. `stars` or `views_count`
fn read_sort(input: &str) -> RepoSort {
  let input = input.trim();
  if input.is_empty() {
    return RepoSort::default();
  }

  let de = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(input);
  match RepoSort::deserialize(de) {
    Ok(x) if !matches!(x, RepoSort::Name | RepoSort::Watchers) => x,
    _ => {
      tracing::warn!("invalid GHS_SORT, ignored: {input:?}");
      RepoSort::default()
    }
  }
}

fn env_bool(key: &str) -> bool {
  let val = std::env::var(key).unwrap_or_else(|_| "false".to_string()).to_lowercase();
  val == "true" || val == "1"
}

#[derive(Clone)]
pub struct DbHealth {
  pub checked_at: Instant,
  pub result: Result<(), String>,
}

#[derive(Clone, Default)]
pub struct SyncStatus {
  pub last_success: Option<DateTime<Utc>>,
  pub last_error: Option<String>,
  pub running: bool,
  pub started: Option<DateTime<Utc>>,
  pub phase: &'static str,
  pub phase_started: Option<DateTime<Utc>>,
  pub done: usize,
  pub total: usize,
  /// items of the whole job (planned, grows if a phase is bigger), done in finished phases
  pub job_total: usize,
  pub job_done: usize,
  pub pct: usize,
}

impl SyncStatus {
  /// The job's percent never goes back when a phase turns out bigger than planned
  pub fn advance(&mut self, phase: &'static str, done: usize, total: usize) {
    if self.phase != phase {
      self.job_done += self.total;
      self.phase_started = Some(Utc::now());
    }

    (self.phase, self.done, self.total) = (phase, done, total);
    self.job_total = self.job_total.max(self.job_done + total).max(1);
    self.pct = self.pct.max(((self.job_done + done) * 100 / self.job_total).min(99));
  }
}

impl DbHealth {
  fn new(result: Result<(), String>) -> Self {
    Self { checked_at: Instant::now(), result }
  }
}

pub struct AppState {
  pub db: DbClient,
  pub gh: GhClient,
  pub filter: Filter,
  pub show: Filter,
  pub include_private: bool,
  pub version_check: bool,
  pub default_sort: RepoSort,
  pub last_release: Mutex<String>,
  db_health: Mutex<DbHealth>,
  sync_status: Mutex<SyncStatus>,
}

impl AppState {
  pub async fn new() -> Res<Self> {
    let gh_token_file = std::env::var("GITHUB_TOKEN_FILE").unwrap_or_default();
    let gh_token = std::env::var("GITHUB_TOKEN").unwrap_or_default();
    if gh_token_file.is_empty() && gh_token.is_empty() {
      tracing::error!("either GITHUB_TOKEN or GITHUB_TOKEN_FILE must be provided");
      std::process::exit(1);
    }

    let filter = read_filter(&std::env::var("GHS_FILTER").unwrap_or_default());
    let show = std::env::var("GHS_SHOW").unwrap_or_default();
    tracing::info!("GHS_SHOW: {show:?}");
    let show = parse_or_all("GHS_SHOW", &show, true);

    let db_path = std::env::var("DB_PATH").unwrap_or("./data/ghstats.db".to_string());
    tracing::info!("db_path: {}", db_path);

    let db = DbClient::new(&db_path).await?;
    if let Err(e) = db.check_writable().await {
      let details = e.to_string();
      tracing::error!("{}: {}", DB_NOT_WRITABLE_MESSAGE, details);
      anyhow::bail!("{}: {}", DB_NOT_WRITABLE_MESSAGE, details);
    }

    let gh = match gh_token_file.is_empty() {
      true => GhClient::new(gh_token)?,
      false => GhClient::from_token_file(gh_token_file)?,
    };

    let include_private = env_bool("GHS_INCLUDE_PRIVATE");
    let version_check = !env_bool("GHS_DISABLE_VERSION_CHECK");
    let default_sort = read_sort(&std::env::var("GHS_SORT").unwrap_or_default());

    let last_release = Mutex::new(env!("CARGO_PKG_VERSION").to_string());
    let db_health = Mutex::new(DbHealth::new(Ok(())));
    let sync_status = Mutex::new(SyncStatus::default());
    Ok(Self {
      db,
      gh,
      filter,
      show,
      include_private,
      version_check,
      default_sort,
      last_release,
      db_health,
      sync_status,
    })
  }

  pub fn db_health(&self) -> DbHealth {
    self.db_health.lock().unwrap().clone()
  }

  pub async fn update_db_health(&self) {
    let result = self.db.check_writable().await.map_err(|e| e.to_string());
    if let Err(e) = &result {
      tracing::error!("{}: {}", DB_NOT_WRITABLE_MESSAGE, e);
    }

    *self.db_health.lock().unwrap() = DbHealth::new(result);
  }

  pub fn sync_status(&self) -> SyncStatus {
    self.sync_status.lock().unwrap().clone()
  }

  pub fn record_sync_start(&self) {
    let mut status = self.sync_status.lock().unwrap();
    let (last_success, last_error) = (status.last_success, status.last_error.take());
    *status = SyncStatus {
      running: true,
      started: Some(Utc::now()),
      last_success,
      last_error,
      ..Default::default()
    };
  }

  pub fn record_sync_plan(&self, items: usize) {
    self.sync_status.lock().unwrap().job_total = items;
  }

  pub fn record_sync_progress(&self, phase: &'static str, done: usize, total: usize) {
    self.sync_status.lock().unwrap().advance(phase, done, total);
  }

  pub fn record_sync_success(&self) {
    *self.sync_status.lock().unwrap() =
      SyncStatus { last_success: Some(Utc::now()), ..Default::default() };
  }

  pub fn record_sync_error(&self, error: &anyhow::Error) {
    let mut status = self.sync_status.lock().unwrap();
    status.last_error = Some(error.to_string());
    status.running = false;
  }

  pub async fn get_repos_filtered(&self, qs: &RepoFilter) -> Res<Vec<RepoTotals>> {
    let repos = self.db.get_repos(qs).await?;
    let repos = repos.into_iter().filter(|x| self.is_included(&x.name, x.fork, x.archived));

    let repos: Vec<RepoTotals> = match &qs.q {
      Some(q) if !q.is_empty() => {
        let q = q.to_lowercase();
        repos.filter(|x| x.name.to_lowercase().contains(&q)).collect()
      }
      _ => repos.collect(),
    };

    let repos: Vec<RepoTotals> = match &qs.owner {
      Some(owner) if !owner.is_empty() => {
        repos.into_iter().filter(|x| x.name.split('/').next().is_some_and(|o| o == owner)).collect()
      }
      _ => repos,
    };

    Ok(repos)
  }

  pub fn is_included(&self, name: &str, fork: bool, archived: bool) -> bool {
    self.filter.matches(&Repo { name, fork, archived, ..Default::default() })
  }

  pub fn is_shown(&self, x: &RepoTotals) -> bool {
    let created = x.created_at.as_deref().and_then(|x| x.parse::<DateTime<Utc>>().ok());
    self.show.matches(&Repo {
      name: &x.name,
      fork: x.fork,
      archived: x.archived,
      stars: x.stars.into(),
      forks: x.forks.into(),
      issues: x.issues.into(),
      prs: x.prs.into(),
      age: created.map(|x| (Utc::now() - x).num_days()),
    })
  }

  // the public pages: GHS_SHOW hides repos there, the API and badges still see them
  pub async fn get_repos_shown(&self, qs: &RepoFilter) -> Res<Vec<RepoTotals>> {
    let mut repos = self.get_repos_filtered(qs).await?;
    repos.retain(|x| self.is_shown(x));
    Ok(repos)
  }

  pub async fn get_owners(&self) -> Res<Vec<String>> {
    let filter = RepoFilter::default();
    let repos = self.db.get_repos(&filter).await?;
    let mut owners: Vec<String> = repos
      .iter()
      .filter(|x| self.is_included(&x.name, x.fork, x.archived) && self.is_shown(x))
      .filter_map(|x| x.name.split('/').next().map(|s| s.to_string()))
      .collect();
    owners.sort();
    owners.dedup();
    Ok(owners)
  }

  #[cfg(test)]
  pub(crate) fn for_test(db: DbClient, filter: &str) -> Self {
    Self {
      db,
      gh: GhClient::new("test-token".to_string()).unwrap(),
      filter: read_filter(filter),
      show: Filter::parse("", true).unwrap(),
      include_private: false,
      version_check: false,
      default_sort: RepoSort::default(),
      last_release: Mutex::new(env!("CARGO_PKG_VERSION").to_string()),
      db_health: Mutex::new(DbHealth::new(Ok(()))),
      sync_status: Mutex::new(SyncStatus::default()),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn ghs_sort_reads_url_keys() {
    assert_eq!(read_sort(""), RepoSort::Views);
    assert_eq!(read_sort(" stars "), RepoSort::Stars);
    assert_eq!(read_sort("clones_count"), RepoSort::Clones);
    assert_eq!(read_sort("bogus"), RepoSort::Views, "invalid falls back to the default");
    assert_eq!(read_sort("name"), RepoSort::Views, "name is for the column header only");
    assert_eq!(read_sort("watchers"), RepoSort::Views, "the home table has no watchers column");
  }

  // the GHS_FILTER matcher before the query syntax, kept to check the translation
  struct GhsFilter {
    include_repos: Vec<String>,
    exclude_repos: Vec<String>,
    exclude_forks: bool,
    exclude_archs: bool,
    default_all: bool,
  }

  impl GhsFilter {
    fn new(rules: &str) -> Self {
      let mut default_all = false;
      let mut exclude_forks = false;
      let mut exclude_archs = false;
      let mut include_repos: Vec<&str> = Vec::new();
      let mut exclude_repos: Vec<&str> = Vec::new();

      let rules = rules.trim().to_lowercase();
      for rule in rules.split(",").map(|x| x.trim()) {
        if rule.is_empty() {
          continue;
        }

        if rule == "*" {
          default_all = true;
          continue;
        }

        if rule == "!fork" {
          exclude_forks = true;
          continue;
        }

        if rule == "!archived" {
          exclude_archs = true;
          continue;
        }

        if rule.matches('/').count() != 1 {
          continue;
        }

        if rule.starts_with('!') {
          exclude_repos.push(rule.strip_prefix('!').unwrap());
        } else {
          include_repos.push(rule);
        }
      }

      if exclude_repos.is_empty() && include_repos.is_empty() {
        default_all = true;
      }

      Self {
        include_repos: include_repos.into_iter().map(|x| x.to_string()).collect(),
        exclude_repos: exclude_repos.into_iter().map(|x| x.to_string()).collect(),
        exclude_forks,
        exclude_archs,
        default_all,
      }
    }

    fn is_included(&self, repo: &str, is_fork: bool, is_arch: bool) -> bool {
      let repo = repo.trim().to_lowercase();
      if repo.is_empty()
        || repo.matches('/').count() != 1
        || repo.starts_with('/')
        || repo.ends_with('/')
      {
        return false;
      }

      for (flag, rules) in [(false, &self.exclude_repos), (true, &self.include_repos)] {
        for rule in rules {
          if rule == &repo {
            return flag;
          }

          if (self.exclude_forks && is_fork) || (self.exclude_archs && is_arch) {
            continue;
          }

          if rule.ends_with("/*")
            && repo.starts_with(&rule[..rule.len() - 2])
            && repo.chars().nth(rule.len() - 2) == Some('/')
          {
            return flag;
          }
        }
      }

      if self.exclude_forks && is_fork {
        return false;
      }

      if self.exclude_archs && is_arch {
        return false;
      }

      self.default_all
    }
  }

  #[test]
  fn legacy_filter_keeps_meaning() {
    let rules = [
      // the readme before the query syntax
      "vladkens/macmon,vladkens/ghstats",
      "vladkens/*,foo-org/bar",
      "vladkens/*,!vladkens/apigen-ts",
      "*,!vladkens/apigen-ts,!foo-org/bar",
      "*,!fork",
      "vladkens/*,!fork",
      "*,vladkens/some-fork,!fork",
      "*,!archived",
      // tricky ones
      "",
      ",",
      "*",
      " * ",
      "a/b",
      "a/*",
      "!fork",
      "!ARCHIVED",
      "!fork,vladkens/some-fork",
      "*,!a/b",
      "!a/*,a/b",
      "a/*,!a/b,!fork",
      "A/*,!a/B",
      "VladKens/MacMon,!FORK",
      " a/* , !fork ",
      "a/b,",
      "*,!a/*,!fork,!archived",
      "*,!fork,!archived,a/b",
      "a/*,!fork,!archived,a/b,!a/c",
      "a/b,c/*,!c/d,!fork",
      "!fork,!archived,!a/*,b/*",
      // the old parser ignored the first two kinds and compared the last literally
      "vladkens/macmon,foo",
      "-*,a/b",
      "a/b/c,c/*",
      "a?b/c,!a/b",
    ];

    let names = [
      "a/b",
      "a/c",
      "A/B",
      "aa/b",
      "b/a",
      "c/d",
      "c/e",
      "fork/x",
      "foo-org/bar",
      "foo-org/baz",
      "other/repo",
      "vladkens/apigen-ts",
      "vladkens/ghstats",
      "vladkens/macmon",
      "vladkens/some-fork",
    ];

    for rules in rules {
      let (old, new) = (GhsFilter::new(rules), read_filter(rules));
      for name in names {
        for (fork, arch) in [(false, false), (true, false), (false, true), (true, true)] {
          let want = old.is_included(name, fork, arch);
          let repo = Repo { name, fork, archived: arch, ..Default::default() };
          assert_eq!(new.matches(&repo), want, "{rules:?} {name} {fork} {arch}");
        }
      }
    }
  }

  // the old matcher took only `owner/*` as a glob and collected nothing for excludes only
  #[test]
  fn legacy_filter_improved() {
    let cases = [
      ("!a/b,!c/d", "a/b", false, false),
      ("!a/b,!c/d", "c/d", false, false),
      ("!a/b,!c/d", "a/c", false, true),
      ("!a/*,!fork", "a/b", false, false),
      ("!a/*,!fork", "b/a", false, true),
      ("!a/*,!fork", "b/a", true, false),
      ("*/*,a/b", "b/a", true, true),
      ("f*o/bar,!fork", "foo/bar", false, true),
      ("f*o/bar,!fork", "foo/bar", true, false),
      ("f*o/bar,!fork", "foo/baz", false, false),
      ("vladkens/*,!vladkens/apigen-*", "vladkens/apigen-ts", false, false),
      ("vladkens/*,!vladkens/apigen-*", "vladkens/macmon", false, true),
      ("vladkens/*,!vladkens/apigen-*", "other/repo", false, false),
      ("!fork,vladkens/apigen-*", "vladkens/apigen-ts", true, false),
      ("!fork,vladkens/apigen-ts", "vladkens/apigen-ts", true, true),
    ];

    for (rules, name, fork, want) in cases {
      let repo = Repo { name, fork, ..Default::default() };
      assert_eq!(read_filter(rules).matches(&repo), want, "{rules:?} {name} {fork}");
    }
  }

  #[test]
  fn legacy_query_text() {
    let warn = |x: &str| vec![x.to_string()];
    let cases = [
      ("!fork,vladkens/*", Some(("vladkens/* fork:false", vec![]))),
      ("*,vladkens/some-fork,!fork", Some(("(vladkens/some-fork OR fork:false)", vec![]))),
      ("*,!a/*,!fork", Some(("fork:false !(fork:false a/*)", vec![]))),
      ("vladkens/macmon,foo", Some(("vladkens/macmon", warn("ignored invalid rule `foo`")))),
      ("a/b,a?b/c", Some(("a/b", warn("rule `a?b/c` matches no repo")))),
      ("a?b/c,!a/b", Some(("!*/*", warn("rule `a?b/c` matches no repo")))),
      ("*/*,a/b", Some(("(a/b OR */*)", vec![]))),
      ("!a/b,!c/d", Some(("!a/b !c/d", vec![]))),
      ("vladkens/*,!vladkens/apigen-*", Some(("vladkens/* !vladkens/apigen-*", vec![]))),
      (" !FORK ", Some(("fork:false", vec![]))),
      ("!archived", Some(("archived:false", vec![]))),
      (",", Some(("", vec![]))),
      ("vladkens/* fork:false", None),
      ("vladkens/macmon OR vladkens/ghstats", None),
      ("!vladkens/apigen-ts", None),
      ("vladkens/*", None),
      ("*", Some(("", vec![]))),
      (" * ", Some(("", vec![]))),
      ("!forks", None),
      // a comma in a query goes to the parser, which rejects it
      ("stars:>1,000", None),
      ("vladkens/*,fork:false", None),
      ("(a/b,c/d)", None),
      ("a/b OR c/d,e/f", None),
      ("a/b,c/d AND e/f", None),
    ];

    for (input, want) in cases {
      let want = want.map(|(q, w)| (q.to_string(), w));
      assert_eq!(legacy_query(input), want, "{input}");
    }
  }

  #[test]
  fn invalid_filter_is_ignored() {
    for input in ["stars:>1", "(vladkens/*", "fork:yes", "vladkens/*,fork:false", "vladkens", "-*"]
    {
      let repo = Repo { name: "a/b", fork: true, archived: true, ..Default::default() };
      assert!(read_filter(input).matches(&repo), "{input}");
    }
  }
}
