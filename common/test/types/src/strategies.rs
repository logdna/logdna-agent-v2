use std::collections::HashMap;
use std::convert::TryInto;

use async_trait::async_trait;

use logdna_client::body::{KeyValueMap, Line, LineMeta};
use logdna_client::serialize::{
    IngestLineSerialize, IngestLineSerializeError, SerializeI64, SerializeMap, SerializeStr,
    SerializeUtf8, SerializeValue,
};

use proptest::collection::hash_map;
use proptest::option::of;
use proptest::prelude::*;
use proptest::string::string_regex;

use proptest::collection::vec;

use state::{FileId, GetOffset, Offset};

pub fn random_line_string_vec(
    min_size: usize,
    max_size: usize,
) -> impl Strategy<Value = Vec<String>> {
    vec(
        (min_size..max_size)
            .prop_flat_map(|i| string_regex(&format!("[a-zA-Z0-9]{{1,64}}{}", i)).unwrap()),
        min_size..max_size,
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct OffsetLine {
    pub line: Line,
    pub offset: Option<Offset>,
}

impl OffsetLine {
    pub fn new(line: Line, offset: Option<Offset>) -> Self {
        OffsetLine { line, offset }
    }
}

impl GetOffset for &OffsetLine {
    fn get_key(&self) -> Option<u64> {
        self.offset.map(|o| o.0.ffi())
    }
    fn get_offset(&self) -> Option<(u64, u64)> {
        self.offset.map(|o| (o.1.start, o.1.end))
    }
}

#[async_trait]
impl IngestLineSerialize<String, bytes::Bytes, std::collections::HashMap<String, String>>
    for &OffsetLine
{
    type Ok = ();

    fn has_annotations(&self) -> bool {
        self.line.get_annotations().is_some()
    }
    async fn annotations<'b, S>(
        &mut self,
        writer: &mut S,
    ) -> Result<Self::Ok, IngestLineSerializeError>
    where
        S: SerializeMap<'b, HashMap<String, String>> + std::marker::Send,
    {
        (&self.line).annotations(writer).await
    }
    fn has_app(&self) -> bool {
        self.line.get_app().is_some()
    }
    async fn app<S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        (&self.line).app(writer).await
    }
    fn has_env(&self) -> bool {
        self.line.get_env().is_some()
    }
    async fn env<S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        (&self.line).env(writer).await
    }
    fn has_file(&self) -> bool {
        self.line.get_file().is_some()
    }
    async fn file<S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        (&self.line).file(writer).await
    }
    fn has_host(&self) -> bool {
        self.line.get_host().is_some()
    }
    async fn host<S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        (&self.line).host(writer).await
    }
    fn has_labels(&self) -> bool {
        self.line.get_labels().is_some()
    }
    async fn labels<'b, S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeMap<'b, HashMap<String, String>> + std::marker::Send,
    {
        (&self.line).labels(writer).await
    }
    fn has_level(&self) -> bool {
        self.line.get_level().is_some()
    }
    async fn level<S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeStr<String> + std::marker::Send,
    {
        (&self.line).level(writer).await
    }
    fn has_meta(&self) -> bool {
        self.line.get_meta().is_some()
    }
    async fn meta<S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeValue + std::marker::Send,
    {
        (&self.line).meta(writer).await
    }
    async fn line<S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeUtf8<bytes::Bytes> + std::marker::Send,
    {
        (&self.line).line(writer).await
    }
    async fn timestamp<S>(&mut self, writer: &mut S) -> Result<(), IngestLineSerializeError>
    where
        S: SerializeI64 + std::marker::Send,
    {
        (&self.line).timestamp(writer).await
    }
    fn field_count(&self) -> usize {
        (&self.line).field_count()
    }
}

fn key_value_map_st(max_entries: usize) -> impl Strategy<Value = KeyValueMap> {
    hash_map(
        string_regex(".{1,64}").unwrap(),
        string_regex(".{1,64}").unwrap(),
        0..max_entries,
    )
    .prop_map(move |c| {
        let mut kv_map = KeyValueMap::new();
        for (k, v) in c.into_iter() {
            kv_map = kv_map.add(k, v);
        }
        kv_map
    })
}

//recursive JSON type
fn json_st(depth: u32) -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(|o| serde_json::to_value(o).unwrap()),
        any::<f64>().prop_map(|o| serde_json::to_value(o).unwrap()),
        ".{1,64}".prop_map(|o| serde_json::to_value(o).unwrap()),
    ];
    leaf.prop_recursive(depth, 256, 10, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..10)
                .prop_map(|o| serde_json::to_value(o).unwrap()),
            prop::collection::hash_map(".*", inner, 0..10)
                .prop_map(|o| serde_json::to_value(o).unwrap()),
        ]
    })
}

pub fn offset_st(size: usize) -> impl Strategy<Value = Option<Offset>> {
    proptest::option::of((1..size).prop_flat_map(|o| {
        (
            proptest::num::u8::ANY.prop_map(|k| FileId::from(k as u64)),
            (0..o).prop_map(move |start| (start as u64, o as u64).try_into().unwrap()),
        )
    }))
}

pub fn line_st(
    offset_st: impl Strategy<Value = Option<Offset>>,
) -> impl Strategy<Value = OffsetLine> {
    (
        of(key_value_map_st(5)),
        of(string_regex(".{1,64}").unwrap()),
        of(string_regex(".{1,64}").unwrap()),
        of(string_regex(".{1,64}").unwrap()),
        of(string_regex(".{1,64}").unwrap()),
        of(key_value_map_st(5)),
        of(string_regex(".{1,64}").unwrap()),
        of(json_st(3)),
        string_regex(".{1,64}").unwrap(),
        (0..i64::MAX),
        offset_st,
    )
        .prop_map(
            |(annotations, app, env, file, host, labels, level, meta, line, timestamp, offset)| {
                OffsetLine::new(
                    Line {
                        annotations,
                        app,
                        env,
                        file,
                        host,
                        labels,
                        level,
                        meta,
                        line,
                        timestamp,
                    },
                    offset,
                )
            },
        )
}
