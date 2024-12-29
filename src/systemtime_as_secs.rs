use std::time::SystemTime;

pub fn as_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_else(|err| err.duration())
        .as_secs()
}

pub fn as_millis(t: SystemTime) -> u128 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_else(|err| err.duration())
        .as_millis()
}
