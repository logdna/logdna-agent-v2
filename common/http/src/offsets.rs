use std::iter::Iterator;

pub type Offset = (u64, u64);

#[derive(Debug)]
pub struct OffsetMap {
    inner: vec_collections::VecMap<[Offset; 4]>,
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

    pub fn items_as_ref(&self) -> &[Offset] {
        self.inner.as_ref()
    }
}

pub struct IntoIter {
    inner: smallvec::IntoIter<[Offset; 4]>,
}

impl IntoIterator for OffsetMap {
    type IntoIter = IntoIter;
    type Item = Offset;
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            inner: self.inner.into_inner().into_iter(),
        }
    }
}

impl Iterator for IntoIter {
    type Item = Offset;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl std::default::Default for OffsetMap {
    fn default() -> Self {
        Self::new()
    }
}
