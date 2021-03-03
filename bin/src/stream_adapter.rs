use async_trait::async_trait;

use fs::cache::tailed_file::LazyLineSerializer;
use http::types::body::{Line, LineBuilder, LineMeta};
use http::types::serialize::{
    IngestLineSerialize, IngestLineSerializeError, SerializeI64, SerializeMap, SerializeStr,
    SerializeUtf8, SerializeValue,
};
use std::collections::HashMap;

pub(crate) enum StrictOrLazyLineBuilder {
    Strict(LineBuilder),
    Lazy(LazyLineSerializer),
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum StrictOrLazyLines<'a> {
    Strict(&'a Line),
    Lazy(LazyLineSerializer),
}

#[async_trait]
impl IngestLineSerialize<String, bytes::Bytes, std::collections::HashMap<String, String>>
    for StrictOrLazyLines<'_>
{
    type Ok = ();

    fn has_annotations(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_annotations().is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_annotations().is_some(),
        }
    }
    async fn annotations<'b, S>(
        &mut self,
        writer: &mut S,
    ) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeMap<'b, HashMap<String, String>> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.annotations(writer).await,
            StrictOrLazyLines::Lazy(line) => line.annotations(writer).await,
        }
    }
    fn has_app(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_app().is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_app().is_some(),
        }
    }
    async fn app<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.app(writer).await,
            StrictOrLazyLines::Lazy(line) => line.app(writer).await,
        }
    }
    fn has_env(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_env().is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_env().is_some(),
        }
    }
    async fn env<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.env(writer).await,
            StrictOrLazyLines::Lazy(line) => line.env(writer).await,
        }
    }
    fn has_file(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_file().is_some(),
            StrictOrLazyLines::Lazy(_) => true,
        }
    }
    async fn file<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.file(writer).await,
            StrictOrLazyLines::Lazy(line) => line.file(writer).await,
        }
    }
    fn has_host(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_host().is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_host().is_some(),
        }
    }
    async fn host<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.host(writer).await,
            StrictOrLazyLines::Lazy(line) => line.host(writer).await,
        }
    }
    fn has_labels(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_labels().is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_labels().is_some(),
        }
    }
    async fn labels<'b, S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeMap<'b, HashMap<String, String>> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.labels(writer).await,
            StrictOrLazyLines::Lazy(line) => line.labels(writer).await,
        }
    }
    fn has_level(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_level().is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_level().is_some(),
        }
    }
    async fn level<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.level(writer).await,
            StrictOrLazyLines::Lazy(line) => line.level(writer).await,
        }
    }
    fn has_meta(&self) -> bool {
        match self {
            StrictOrLazyLines::Strict(line) => line.get_meta().is_some(),
            StrictOrLazyLines::Lazy(line) => line.get_meta().is_some(),
        }
    }
    async fn meta<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeValue + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.meta(writer).await,
            StrictOrLazyLines::Lazy(line) => line.meta(writer).await,
        }
    }
    async fn line<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeUtf8<bytes::Bytes> + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.line(writer).await,
            StrictOrLazyLines::Lazy(line) => line.line(writer).await,
        }
    }
    async fn timestamp<S>(&mut self, writer: &mut S) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeI64 + std::marker::Send,
    {
        match self {
            StrictOrLazyLines::Strict(line) => line.timestamp(writer).await,
            StrictOrLazyLines::Lazy(line) => line.timestamp(writer).await,
        }
    }
    fn field_count(&self) -> usize {
        match self {
            StrictOrLazyLines::Strict(line) => line.field_count(),
            StrictOrLazyLines::Lazy(line) => line.field_count(),
        }
    }
}
