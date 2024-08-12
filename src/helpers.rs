use std::collections::HashMap;

use crate::{
  db_client::DbClient,
  gh_client::{GhClient, Repo},
  types::Res,
};

pub async fn update_repo_metrics(db: &DbClient, gh: &GhClient, repo: &Repo, date: &str) -> Res {
  let views = gh.traffic_views(&repo.full_name).await?;
  let clones = gh.traffic_clones(&repo.full_name).await?;
  let referrers = gh.traffic_refs(&repo.full_name).await?;
  let popular_paths = gh.traffic_paths(&repo.full_name).await?;

  db.insert_stats(&repo, date).await?;
  db.insert_views(&repo, &views).await?;
  db.insert_clones(&repo, &clones).await?;
  db.insert_referrers(&repo, date, &referrers).await?;
  db.insert_paths(&repo, date, &popular_paths).await?;

  Ok(())
}

pub async fn get_stars_history(gh: &GhClient, repo: &str) -> Res<Vec<(String, u32)>> {
  let stars = gh.get_stars(repo).await?;

  let mut dat: HashMap<String, u32> = HashMap::new();
  for star in stars {
    let date = star.starred_at.split("T").next().unwrap().to_owned();
    dat.entry(date).and_modify(|e| *e += 1).or_insert(1);
  }

  let mut dat = dat.into_iter().collect::<Vec<_>>();
  dat.sort_by(|a, b| a.0.cmp(&b.0));

  for i in 1..dat.len() {
    dat[i].1 += dat[i - 1].1;
  }

  Ok(dat)
}

pub async fn sync_stars(db: &DbClient, gh: &GhClient) -> Res {
  let repos = db.repos_to_sync().await?;
  for repo in repos {
    let stars = match get_stars_history(gh, &repo.name).await {
      Ok(stars) => stars,
      Err(e) => {
        tracing::warn!("failed to get stars for {}: {:?}", repo.name, e);
        break;
      }
    };

    db.insert_stars(&repo.name, &stars).await?;
  }

  Ok(())
}
