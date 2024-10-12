use std::collections::HashMap;

use axum::extract::{Query, Request};

use crate::{
  db_client::{DbClient, RepoFilter, RepoTotals},
  gh_client::{GhClient, Repo},
  types::Res,
};

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

pub async fn update_metrics(db: &DbClient, gh: &GhClient) -> Res {
  let stime = std::time::Instant::now();

  let date = chrono::Utc::now().to_utc().to_rfc3339();
  let date = date.split("T").next().unwrap().to_owned() + "T00:00:00Z";

  let repos = gh.get_repos().await?;
  let _ = check_hidden_repos(db, &repos).await?;

  let repos = repos.into_iter().filter(|r| is_repo_included(&r.full_name)).collect::<Vec<_>>();
  for repo in &repos {
    match update_repo_metrics(db, gh, &repo, &date).await {
      Err(e) => {
        tracing::warn!("failed to update metrics for {}: {:?}", repo.full_name, e);
        continue;
      }
      // Ok(_) => tracing::info!("updated metrics for {}", repo.full_name),
      Ok(_) => {}
    }
  }

  tracing::info!("update_metrics took {:?} for {} repos", stime.elapsed(), repos.len());
  db.update_deltas().await?;
  sync_stars(db, gh).await?;

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

pub fn is_repo_included(repo: &str) -> bool {
  let rules = std::env::var("GHS_FILTER").unwrap_or_default();
  is_included(repo, &rules)
}

fn is_included(repo: &str, rules: &str) -> bool {
  let repo = repo.trim().to_lowercase();
  if repo.is_empty()
    || repo.matches('/').count() != 1
    || repo.starts_with('/')
    || repo.ends_with('/')
  {
    return false;
  }

  let rules = rules.trim().to_lowercase();
  let rules: Vec<_> = rules
    .split(",")
    .map(|f| f.trim())
    .filter(|f| {
      if f.is_empty() {
        return false;
      }

      return *f == "*" || f.matches('/').count() == 1;
    })
    .collect();

  if rules.is_empty() {
    return true;
  }

  let exclude: Vec<&str> = rules.iter().filter_map(|x| x.strip_prefix('!')).collect();
  let include: Vec<&str> = rules.iter().filter(|&&x| !x.starts_with('!')).cloned().collect();

  for (flag, rules) in vec![(false, exclude), (true, include)] {
    for rule in rules {
      if rule == repo || rule == "*" {
        return flag;
      }

      if rule.ends_with("/*") && repo.starts_with(&rule[..rule.len() - 2]) {
        return flag;
      }
    }
  }

  return false;
}

pub async fn get_filtered_repos(db: &DbClient, qs: &Query<RepoFilter>) -> Res<Vec<RepoTotals>> {
  let repos = db.get_repos(&qs).await?;
  let repos = repos.into_iter().filter(|x| is_repo_included(&x.name)).collect::<Vec<_>>();
  Ok(repos)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_included_with_empty_env() {
    let r = "";
    assert_eq!(is_included("foo/bar", r), true);
    assert_eq!(is_included("foo/baz", r), true);
    assert_eq!(is_included("abc/123", r), true);
    assert_eq!(is_included("abc/xyz-123", r), true);
    // negative tests – non repo patterns
    assert_eq!(is_included("foo/", r), false);
    assert_eq!(is_included("/bar", r), false);
    assert_eq!(is_included("foo", r), false);
    assert_eq!(is_included("foo/bar/baz", r), false);
    // assert_eq!(is_repo_included("*", r), false);
    // assert_eq!(is_repo_included("foo/*", r), false);
    // assert_eq!(is_repo_included("*/bar", r), false);
  }

  #[test]
  fn test_included_with_env() {
    let r = "foo/*,abc/xyz";
    assert_eq!(is_included("foo/bar", r), true);
    assert_eq!(is_included("foo/abc", r), true);
    assert_eq!(is_included("foo/abc-123", r), true);
    assert_eq!(is_included("abc/xyz", r), true);
    assert_eq!(is_included("abc/123", r), false);
    assert_eq!(is_included("foo/bar/baz", r), false);

    // check case sensitivity
    assert_eq!(is_included("FOO/BAR", r), true);
    assert_eq!(is_included("Foo/Bar", r), true);

    let r = "FOO/*,Abc/XYZ";
    assert_eq!(is_included("foo/bar", r), true);
    assert_eq!(is_included("foo/abc", r), true);
    assert_eq!(is_included("foo/abc-123", r), true);
    assert_eq!(is_included("abc/xyz", r), true);
  }

  #[test]
  fn test_include_with_exclude_rule() {
    let r = "foo/*,!foo/bar";
    assert_eq!(is_included("foo/bar", r), false);
    assert_eq!(is_included("FOO/Bar", r), false);

    assert_eq!(is_included("foo/abc", r), true);
    assert_eq!(is_included("foo/abc-123", r), true);
    assert_eq!(is_included("abc/xyz", r), false);

    let r = "foo/*,!foo/bar,!foo/baz,abc/xyz";
    assert_eq!(is_included("foo/bar", r), false);
    assert_eq!(is_included("foo/baz", r), false);
    assert_eq!(is_included("abc/xyz", r), true);
    assert_eq!(is_included("foo/123", r), true);
  }

  #[test]
  fn test_include_all_expect() {
    let r = "*";
    assert_eq!(is_included("foo/bar", r), true);
    assert_eq!(is_included("abc/123", r), true);

    let r = "-*";
    assert_eq!(is_included("foo/bar", r), true);
    assert_eq!(is_included("abc/123", r), true);

    let r = "*,!foo/bar,!abc/123";
    assert_eq!(is_included("foo/bar", r), false);
    assert_eq!(is_included("abc/123", r), false);
    assert_eq!(is_included("foo/baz", r), true);
    assert_eq!(is_included("abc/xyz", r), true);

    let r = "*,!foo/*";
    assert_eq!(is_included("foo/bar", r), false);
    assert_eq!(is_included("foo/baz", r), false);
    assert_eq!(is_included("abc/123", r), true);
    assert_eq!(is_included("abc/xyz", r), true);
  }
}
