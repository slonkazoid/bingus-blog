use std::time::Duration;

use chrono::{DateTime, TimeZone};

pub fn date<T: TimeZone>(date: &DateTime<T>) -> Result<String, askama::Error> {
    Ok(date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

pub fn duration(duration: &&Duration) -> Result<String, askama::Error> {
    Ok(format!("{:?}", duration))
}
