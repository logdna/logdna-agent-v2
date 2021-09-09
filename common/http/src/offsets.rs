use std::iter::Iterator;

#[derive(Debug)]
pub struct OffsetMap {
    inner: vec_collections::VecMap<[(u64, u64); 4]>,
}

impl OffsetMap {
    fn new() -> Self {
        Self {
            inner: vec_collections::VecMap::default(),
        }
    }

    pub fn insert(&mut self, key: u64, value: u64) -> Option<u64> {
        self.inner.insert(key, value)
    }

    pub fn items_as_ref(&self) -> &[(u64, u64)] {
        self.inner.as_ref()
    }
}

pub struct IntoIter {
    inner: smallvec::IntoIter<[(u64, u64); 4]>,
}

impl IntoIterator for OffsetMap {
    type IntoIter = IntoIter;
    type Item = (u64, u64);
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            inner: self.inner.into_inner().into_iter(),
        }
    }
}

impl Iterator for IntoIter {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl std::default::Default for OffsetMap {
    fn default() -> Self {
        Self::new()
    }
}
