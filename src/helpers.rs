use std::fmt::Display;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use handlebars::handlebars_helper;

use crate::config::DateFormat;

fn date_impl<T>(date_time: &DateTime<T>, date_format: &DateFormat) -> String
where
    T: TimeZone,
    T::Offset: Display,
{
    match date_format {
        DateFormat::RFC3339 => date_time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        DateFormat::Strftime(ref format_string) => date_time.format(format_string).to_string(),
    }
}

handlebars_helper!(date: |date_time: Option<DateTime<Utc>>, date_format: DateFormat| {
    date_impl(date_time.as_ref().unwrap(), &date_format)
});

handlebars_helper!(duration: |duration_: Duration| format!("{:?}", duration_));
