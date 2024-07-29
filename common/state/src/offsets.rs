use std::sync::Arc;

use thiserror::Error;

use crate::{FileId, Span, SpanVec};

use serde::{Deserialize, Serialize};

use vec_collections::AbstractVecMap;

pub type Offset = (FileId, Span);

#[derive(Debug, Error)]
pub enum OffsetMapError {
    #[error("{0}")]
    SpanError(crate::SpanError),
    #[error("OffsetMap is not unique, cannot modify OffsetMap with clones.")]
    NonUnique,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct OffsetMap {
    inner: Arc<vec_collections::VecMap<[(FileId, SpanVec); 4]>>,
}

impl OffsetMap {
    fn new() -> Self {
        Self {
            inner: Arc::new(vec_collections::VecMap::default()),
        }
    }

    pub fn insert(&mut self, key: u64, value: Span) -> Result<(), OffsetMapError> {
        let map = Arc::get_mut(&mut self.inner).ok_or(OffsetMapError::NonUnique)?;

        let key = FileId::from(key);
        if let Some(span_v) = map.get_mut(&key) {
            span_v.insert(value);
        } else {
            let mut span_v = SpanVec::new();
            span_v.insert(value);
            map.insert(key, span_v);
        }
        Ok(())
    }

    pub fn items_as_ref(&self) -> &[(FileId, SpanVec)] {
        self.inner.as_ref().as_ref()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(FileId, SpanVec)> {
        self.inner.as_ref().iter()
    }

    pub fn into_inner(self) -> Arc<vec_collections::VecMap<[(FileId, SpanVec); 4]>> {
        self.inner
    }
}

impl std::default::Default for OffsetMap {
    fn default() -> Self {
        Self::new()
    }
}
