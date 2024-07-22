pub type WithError<T> = Result<T, Box<dyn std::error::Error + Sync + Send>>;
