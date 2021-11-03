use std::convert::TryInto;
use std::sync::Arc;

use thiserror::Error;

use crate::{FileId, Span};

use serde::{Deserialize, Serialize};

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
    inner: Arc<vec_collections::VecMap<[Offset; 4]>>,
}

impl OffsetMap {
    fn new() -> Self {
        Self {
            inner: Arc::new(vec_collections::VecMap::default()),
        }
    }

    pub fn insert(
        &mut self,
        key: u64,
        value: (u64, u64),
    ) -> Result<Option<(u64, u64)>, OffsetMapError> {
        Ok(Arc::get_mut(&mut self.inner)
            .ok_or(OffsetMapError::NonUnique)?
            .insert(
                FileId::from(key),
                value.try_into().map_err(OffsetMapError::SpanError)?,
            )
            .map(|span| (span.start, span.end)))
    }

    pub fn items_as_ref(&self) -> &[Offset] {
        self.inner.as_ref().as_ref()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Offset> {
        self.inner.as_ref().iter()
    }

    pub fn into_inner(self) -> Arc<vec_collections::VecMap<[Offset; 4]>> {
        self.inner
    }
}

impl std::default::Default for OffsetMap {
    fn default() -> Self {
        Self::new()
    }
}
