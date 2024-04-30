use std::time::SystemTime;

pub fn as_secs(t: &SystemTime) -> u64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration,
        Err(err) => err.duration(),
    }
    .as_secs()
}
