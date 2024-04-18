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

    /*pub fn get(&self, key: &Lookup) -> Option<Arc<T>> {
        self.hash.and_then(|hash| {
            let mut h = DefaultHasher::new();
            key.hash(&mut h);
            if hash == h.finish() {
                self.inner.clone()
            } else {
                None
            }
        })
    }*/

    pub fn get_or_init(&mut self, key: &Lookup, init: impl Fn(&Lookup) -> Arc<T>) -> Arc<T> {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        let hash = h.finish();
        if !self.hash.is_some_and(|inner_hash| inner_hash == hash) {
            let mut h = DefaultHasher::new();
            key.hash(&mut h);
            self.inner = Some(init(key));
            self.hash = Some(h.finish());
        }
        // safety: please.
        unsafe { self.inner.as_ref().unwrap_unchecked().clone() }
    }
}
