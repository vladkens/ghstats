use chrono::Timelike;

pub type WithError<T = ()> = Result<T, Box<dyn std::error::Error + Sync + Send>>;

pub fn get_utc_hours() -> chrono::DateTime<chrono::Utc> {
  let ts = chrono::Utc::now().to_utc();
  let ts = ts.with_minute(0).unwrap().with_second(0).unwrap().with_nanosecond(0).unwrap();
  ts
}
