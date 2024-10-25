use std::{collections::HashMap, sync::Arc};

use axum::extract::Request;

use crate::{
  db_client::DbClient,
  gh_client::{GhClient, Repo},
  state::AppState,
  types::Res,
};

pub fn truncate_middle(text: &str, max_len: usize) -> String {
  if text.len() <= max_len {
    return text.to_string();
  }

  let part_len = (max_len - 3) / 2;
  let start = &text[..part_len];
  let end = &text[text.len() - part_len..];

  format!("{}...{}", start, end)
}

pub fn get_header<'a>(req: &'a Request, name: &'a str) -> Option<&'a str> {
  match req.headers().get(name) {
    Some(x) => Some(x.to_str().unwrap_or_default()),
    None => None,
  }
}

async fn check_hidden_repos(db: &DbClient, repos: &Vec<Repo>) -> Res {
  let now_ids = repos.iter().map(|r| r.id as i64).collect::<Vec<_>>();
  let was_ids = db.get_repos_ids().await?;
  let hidden = was_ids.into_iter().filter(|id| !now_ids.contains(id)).collect::<Vec<_>>();
  let _ = db.mark_repo_hidden(&hidden).await?;

  Ok(())
}

pub async fn update_metrics(state: Arc<AppState>) -> Res {
  let stime = std::time::Instant::now();

  let date = chrono::Utc::now().to_utc().to_rfc3339();
  let date = date.split("T").next().unwrap().to_owned() + "T00:00:00Z";

  let repos = state.gh.get_repos(state.include_private).await?;
  let _ = check_hidden_repos(&state.db, &repos).await?;

  let repos = repos //
    .iter()
    .filter(|r| state.filter.is_included(&r.full_name, r.fork, r.archived))
    .collect::<Vec<_>>();

  for repo in &repos {
    match update_repo_metrics(&state.db, &state.gh, &repo, &date).await {
      Err(e) => {
        tracing::warn!("failed to update metrics for {}: {:?}", repo.full_name, e);
        continue;
      }
      // Ok(_) => tracing::info!("updated metrics for {}", repo.full_name),
      Ok(_) => {}
    }
  }

  tracing::info!("update_metrics took {:?} for {} repos", stime.elapsed(), repos.len());
  state.db.update_deltas().await?;
  sync_stars(&state.db, &state.gh).await?;

  Ok(())
}

async fn update_repo_metrics(db: &DbClient, gh: &GhClient, repo: &Repo, date: &str) -> Res {
  let prs = gh.get_open_pull_requests(&repo.full_name).await?;
  let views = gh.traffic_views(&repo.full_name).await?;
  let clones = gh.traffic_clones(&repo.full_name).await?;
  let referrers = gh.traffic_refs(&repo.full_name).await?;

  let popular_paths = gh.traffic_paths(&repo.full_name).await?;

  db.insert_repo(&repo).await?;
  db.insert_stats(&repo, date, &prs).await?;
  db.insert_views(&repo, &views).await?;
  db.insert_clones(&repo, &clones).await?;
  db.insert_referrers(&repo, date, &referrers).await?;
  db.insert_paths(&repo, date, &popular_paths).await?;

  Ok(())
}

/// Get stars history for a repo
/// vec![(date_str, acc_stars, new_stars)), ...]
pub async fn get_stars_history(gh: &GhClient, repo: &str) -> Res<Vec<(String, u32, u32)>> {
  let stars = gh.get_stars(repo).await?;

  let mut dat: HashMap<String, u32> = HashMap::new();
  for star in stars {
    let date = star.starred_at.split("T").next().unwrap().to_owned();
    let date = format!("{date}T00:00:00Z"); // db stores dates as UTC midnight
    dat.entry(date).and_modify(|e| *e += 1).or_insert(1);
  }

  let mut dat = dat.into_iter().collect::<Vec<_>>();
  dat.sort_by(|a, b| a.0.cmp(&b.0));

  let mut rs: Vec<(String, u32, u32)> = Vec::with_capacity(dat.len());
  for i in 0..dat.len() {
    let (date, new_count) = &dat[i];
    let acc_count = if i > 0 { rs[i - 1].1 + new_count } else { new_count.clone() };
    rs.push((date.clone(), acc_count, new_count.clone()));
  }

  Ok(rs)
}

pub async fn sync_stars(db: &DbClient, gh: &GhClient) -> Res {
  let mut pages_collected = 0;

  let repos = db.repos_to_sync().await?;
  for repo in repos {
    let stime = std::time::Instant::now();
    // tracing::info!("sync_stars for {}", repo.name);

    let stars = match get_stars_history(gh, &repo.name).await {
      Ok(stars) => stars,
      Err(e) => {
        tracing::warn!("failed to get stars for {}: {:?}", repo.name, e);
        break;
      }
    };

    db.insert_stars(repo.id, &stars).await?;
    db.mark_repo_stars_synced(repo.id).await?;

    let stars_count = stars.iter().map(|(_, _, c)| c).sum::<u32>();
    tracing::info!(
      "sync_stars for {} done in {:?}, {stars_count} starts added",
      repo.name,
      stime.elapsed(),
    );

    // gh api rate limit is 5000 req/h, so this code will do up to 1000 req/h
    // to not block other possible user pipelines
    pages_collected += (stars_count + 99) / 100;
    if pages_collected > 1000 {
      tracing::info!("sync_stars: {} pages collected, will continue next hour", pages_collected);
      break;
    }
  }

  Ok(())
}

#[derive(Debug)]
pub struct GhsFilter {
  pub include_repos: Vec<String>,
  pub exclude_repos: Vec<String>,
  pub exclude_forks: bool,
  pub exclude_archs: bool,
  pub default_all: bool,
}

impl GhsFilter {
  pub fn new(rules: &str) -> Self {
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

    // if no repo rules, include all by default
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

  pub fn is_included(&self, repo: &str, is_fork: bool, is_arch: bool) -> bool {
    let repo = repo.trim().to_lowercase();
    if repo.is_empty()
      || repo.matches('/').count() != 1
      || repo.starts_with('/')
      || repo.ends_with('/')
    {
      return false;
    }

    for (flag, rules) in vec![(false, &self.exclude_repos), (true, &self.include_repos)] {
      for rule in rules {
        if rule == &repo {
          return flag;
        }

        // skip wildcards for forks / archived
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

    return self.default_all;
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_empty_fitler() {
    let r = &GhsFilter::new("");

    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("foo/baz", false, false));
    assert!(r.is_included("abc/123", false, false));
    assert!(r.is_included("abc/xyz-123", false, false));

    // exclude invalid names
    assert!(!r.is_included("foo/", false, false));
    assert!(!r.is_included("/bar", false, false));
    assert!(!r.is_included("foo", false, false));
    assert!(!r.is_included("foo/bar/baz", false, false));

    // include forks / archived
    assert!(r.is_included("foo/bar", true, false));
    assert!(r.is_included("foo/bar", false, true));
    assert!(r.is_included("foo/bar", true, true));
  }

  #[test]
  fn test_filter_names() {
    let r = &GhsFilter::new("foo/*,abc/xyz");

    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("foo/123", false, false));
    assert!(r.is_included("abc/xyz", false, false));

    assert!(!r.is_included("foo/bar/baz", false, false));
    assert!(!r.is_included("abc/123", false, false));

    // include forks / archived
    assert!(r.is_included("foo/bar", true, false));
    assert!(r.is_included("foo/bar", false, true));

    // exact org/user match
    let r = &GhsFilter::new("foo/*");
    assert!(!r.is_included("fooo/bar", false, false));
  }

  #[test]
  fn test_filter_names_case() {
    let r = &GhsFilter::new("foo/*,abc/xyz");
    assert!(r.is_included("FOO/BAR", false, false));
    assert!(r.is_included("Foo/Bar", false, false));

    let r = &GhsFilter::new("FOO/*,Abc/XYZ");
    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("foo/baz", false, false));
    assert!(r.is_included("abc/xyz", false, false));
  }

  #[test]
  fn test_filter_all_expect() {
    let r = &GhsFilter::new("*");
    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("abc/123", false, false));
    assert!(r.is_included("abc/123", true, false));
    assert!(r.is_included("abc/123", true, true));

    let r = &GhsFilter::new("-*"); // single rule invalid, include all
    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("abc/123", false, false));

    let r = &GhsFilter::new("*,!foo/bar,!abc/123");
    assert!(!r.is_included("foo/bar", false, false));
    assert!(!r.is_included("abc/123", false, false));
    assert!(r.is_included("foo/baz", false, false));
    assert!(r.is_included("abc/xyz", false, false));

    let r = &GhsFilter::new("*,!foo/*");
    assert!(!r.is_included("foo/bar", false, false));
    assert!(!r.is_included("foo/baz", false, false));
    assert!(r.is_included("abc/123", false, false));
    assert!(r.is_included("abc/xyz", false, false));
  }

  #[test]
  fn test_filter_names_only() {
    let r = &GhsFilter::new("foo/*,!foo/bar");
    assert!(!r.is_included("abc/xyz", false, false));
    assert!(!r.is_included("foo/bar", false, false));
    assert!(!r.is_included("FOO/Bar", false, false));

    assert!(r.is_included("foo/abc", false, false));
    assert!(r.is_included("foo/abc", true, false));
    assert!(r.is_included("foo/abc", true, true));

    let r = &GhsFilter::new("foo/*,!foo/bar,!foo/baz,abc/xyz");
    assert!(!r.is_included("foo/bar", false, false));
    assert!(!r.is_included("foo/baz", false, false));
    assert!(!r.is_included("abc/123", false, false));

    assert!(r.is_included("foo/123", false, false));
    assert!(r.is_included("foo/123", true, false));
    assert!(r.is_included("foo/123", false, true));

    assert!(r.is_included("abc/xyz", false, false));
    assert!(r.is_included("abc/xyz", true, false));
    assert!(r.is_included("abc/xyz", false, true));
  }

  #[test]
  fn test_filter_meta() {
    let r = &GhsFilter::new("*,!fork,!archived,foo/baz");
    assert!(r.exclude_forks);
    assert!(r.exclude_archs);
    assert!(r.default_all);

    assert!(r.is_included("foo/bar", false, false));
    assert!(!r.is_included("foo/bar", true, false));
    assert!(!r.is_included("foo/bar", false, true));

    assert!(r.is_included("abc/123", false, false));
    assert!(!r.is_included("abc/123", true, false));
    assert!(!r.is_included("abc/123", false, true));

    // explicitly added
    assert!(r.is_included("foo/baz", false, false));
    assert!(r.is_included("foo/baz", true, false));
    assert!(r.is_included("foo/baz", false, true));
  }

  #[test]
  fn test_filter_meta_wildcard() {
    let r = &GhsFilter::new("!fork,abc/*,abc/xyz");
    assert!(!r.is_included("abc/123", true, false)); // no wildcard for forks
    assert!(r.is_included("abc/xyz", true, false)); // explicitly added

    let r = &GhsFilter::new("!archived,abc/*,abc/xyz");
    assert!(!r.is_included("abc/123", false, true)); // no wildcard for archived
    assert!(r.is_included("abc/xyz", false, true)); // explicitly added
  }

  #[test]
  fn test_issue18() {
    // test order of rules not affecting the result
    let rules = vec!["foo/*,!foo/bar", "!foo/bar,foo/*"];
    for r in rules {
      let r = &GhsFilter::new(r);
      assert!(!r.is_included("foo/bar", false, false)); // explicitly excluded
      assert!(!r.is_included("abc/abc", false, false)); // not included by default
      assert!(r.is_included("foo/baz", false, false)); // wildcard included
    }

    let rules = vec!["foo/*,!fork", "!fork,foo/*"];
    for r in rules {
      let r = &GhsFilter::new(r);
      assert!(r.is_included("foo/bar", false, false)); // wildcard included
      assert!(!r.is_included("foo/bar", true, false)); // forks excluded
      assert!(!r.is_included("abc/abc", false, false)); // not included by default
    }
  }
}
