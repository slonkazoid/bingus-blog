use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, TimeZone};

use crate::post::PostMetadata;

pub fn date<T: TimeZone>(date: &DateTime<T>) -> Result<String, askama::Error> {
    Ok(date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

pub fn duration(duration: &&Duration) -> Result<String, askama::Error> {
    Ok(format!("{:?}", duration))
}

pub fn collect_tags(posts: &Vec<PostMetadata>) -> Result<Vec<(String, u64)>, askama::Error> {
    let mut tags = HashMap::new();

    for post in posts {
        for tag in &post.tags {
            if let Some((existing_tag, count)) = tags.remove_entry(tag) {
                tags.insert(existing_tag, count + 1);
            } else {
                tags.insert(tag.clone(), 1);
            }
        }
    }

    let mut tags: Vec<(String, u64)> = tags.into_iter().collect();

    tags.sort_unstable_by_key(|(v, _)| v.clone());
    tags.sort_by_key(|(_, v)| -(*v as i64));

    Ok(tags)
}
