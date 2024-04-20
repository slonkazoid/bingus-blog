use std::hash::{DefaultHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;

pub struct HashArcStore<T, Lookup>
where
    Lookup: Hash,
{
    inner: Option<Arc<T>>,
    hash: Option<u64>,
    _phantom: PhantomData<Lookup>,
}

impl<T, Lookup> HashArcStore<T, Lookup>
where
    Lookup: Hash,
{
    pub fn new() -> Self {
        Self {
            inner: None,
            hash: None,
            _phantom: PhantomData,
        }
    }

    pub fn get_or_init(&mut self, key: &Lookup, init: impl Fn(&Lookup) -> Arc<T>) -> Arc<T> {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        let hash = h.finish();
        if !self.hash.is_some_and(|inner_hash| inner_hash == hash) {
            self.inner = Some(init(key));
            self.hash = Some(hash);
        }
        // safety: please.
        unsafe { self.inner.as_ref().unwrap_unchecked().clone() }
    }
}
