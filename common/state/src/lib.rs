use rocksdb::{ColumnFamilyDescriptor, IteratorMode, Options, WriteBatch, DB};

use derivative::Derivative;
use futures::future::{Future, FutureExt};
use futures::stream::StreamExt;
use slotmap::{DefaultKey, SlotMap};

use log::{error, info, warn};

use async_lock::RwLock;
use std::collections::HashMap;
use std::convert::{AsRef, Into, TryFrom, TryInto};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

const OFFSET_NAME: &str = "file_offsets";

#[derive(Debug, Error)]
pub enum StateError {
    #[error("{0}")]
    RocksDb(#[from] rocksdb::Error),
    #[error("{0}")]
    IoError(#[from] std::io::Error),
    #[error("{0:?}")]
    PermissionDenied(PathBuf),
    #[error("{0:?}")]
    InvalidPath(PathBuf),
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct AgentState {
    #[derivative(Debug = "ignore")]
    db: Arc<DB>,
    #[derivative(Debug = "ignore")]
    offset_cf_opt: Options,
}

// This was the old new fn implementation. It could probably be broken up
// into a few smaller, private functions.
fn _construct_agent_state(path: &Path) -> Result<AgentState, StateError> {
    if path.metadata()?.permissions().readonly() {
        return Err(StateError::PermissionDenied(path.into()));
    }

    let path = path.join("agent_state.db");

    let mut db_opts = Options::default();
    db_opts.create_missing_column_families(true);
    db_opts.create_if_missing(true);

    let offset_cf_opt = Options::default();
    let offset_cf = ColumnFamilyDescriptor::new(OFFSET_NAME, offset_cf_opt.clone());
    let cfs = vec![offset_cf];

    info!("Opening state db at {:?}", path);

    let db = match DB::open_cf_descriptors(&db_opts, &path, cfs) {
        Ok(db) => db,
        // Attempt to repair a badly closed DB
        Err(e) => {
            warn!("error opening state db, attempted to repair: {}", e);
            DB::repair(&db_opts, &path).map_or_else(
                |_| {
                    DB::destroy(&db_opts, &path)?;
                    DB::open_cf_descriptors(
                        &db_opts,
                        &path,
                        vec![ColumnFamilyDescriptor::new(
                            OFFSET_NAME,
                            offset_cf_opt.clone(),
                        )],
                    )
                },
                |_| {
                    DB::open_cf_descriptors(
                        &db_opts,
                        &path,
                        vec![ColumnFamilyDescriptor::new(
                            OFFSET_NAME,
                            offset_cf_opt.clone(),
                        )],
                    )
                },
            )?
        }
    };
    Ok(AgentState {
        db: Arc::new(db),
        offset_cf_opt,
    })
}

impl AgentState {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StateError> {
        let path = path.as_ref();

        if !path.exists() {
            std::fs::create_dir_all(path).map_err(StateError::IoError)?;
        } else if !path.is_dir() {
            error!("{} is not a directory", path.to_string_lossy());
            return Err(StateError::InvalidPath(path.to_path_buf()));
        }

        _construct_agent_state(path)
    }
}

impl AgentState {
    pub fn get_offset_state<'a, 'b>(&'a self) -> FileOffsetState<'b> {
        FileOffsetState::new(self.db.clone(), self.offset_cf_opt.clone())
    }
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub struct FileId(u64);

impl From<&u64> for FileId {
    fn from(i: &u64) -> FileId {
        FileId(*i)
    }
}

impl From<u64> for FileId {
    fn from(i: u64) -> FileId {
        FileId(i)
    }
}

#[derive(Debug, Error)]
pub enum FileOffsetStateError {
    #[error("{0}")]
    UpdateError(#[from] async_channel::SendError<FileOffsetEvent>),
    #[error("{0}")]
    DbError(String),
    #[error("{0}")]
    RocksDb(#[from] rocksdb::Error),
    #[error("FileOffsetState already running")]
    AlreadyRunning,
    #[error("FileOffsetState shutdown handle already taken")]
    ShutdownHandleTaken,
}

type Offset = u64;

pub struct FileOffset {
    pub key: FileId,
    pub offset: u64,
}

impl<'a, T> From<&'a (T, u64)> for FileOffset
where
    FileId: std::convert::From<&'a T>,
{
    fn from(pair: &'a (T, u64)) -> FileOffset {
        let key = (&pair.0).into();
        let offset = pair.1;
        FileOffset { key, offset }
    }
}

pub enum FileOffsetUpdate {
    Update(FileOffset),
    Delete(FileId),
}

pub enum FileOffsetEvent {
    Update(FileOffsetUpdate),
    Clear,
    Flush,
}

#[derive(Clone)]
pub struct FileOffsetWriteHandle {
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetWriteHandle {
    pub async fn update<'a, T>(&self, offsets: &'a [(T, u64)]) -> Result<(), FileOffsetStateError>
    where
        FileId: std::convert::From<&'a T>,
    {
        let os = &offsets[0];
        let os: FileOffset = os.into();

        Ok(self
            .tx
            .send(FileOffsetEvent::Update(FileOffsetUpdate::Update(
                FileOffset {
                    key: os.key,
                    offset: os.offset,
                },
            )))
            .await?)
    }

    pub async fn delete(&self, file_name: impl Into<FileId>) -> Result<(), FileOffsetStateError> {
        Ok(self
            .tx
            .send(FileOffsetEvent::Update(FileOffsetUpdate::Delete(
                file_name.into(),
            )))
            .await?)
    }
}

pub struct FileOffsetFlushHandle {
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetFlushHandle {
    pub async fn flush(&self) -> Result<(), FileOffsetStateError> {
        Ok(self.tx.send(FileOffsetEvent::Flush).await?)
    }

    pub async fn clear(&self) -> Result<(), FileOffsetStateError> {
        Ok(self.tx.send(FileOffsetEvent::Clear).await?)
    }
}

pub struct FileOffsetShutdownHandle {
    tx: async_channel::Sender<FileOffsetEvent>,
}

impl FileOffsetShutdownHandle {
    pub fn shutdown(&self) -> bool {
        self.tx.close()
    }
}

#[derive(Debug, Error)]
enum SpanError {
    #[error("Invalid Span")]
    InvalidSpan,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SpanCoalesced {
    Overlap(Span),
    IdentityOverlap(Span),
    LeftIdentityOverlap(Span, Span),
    RightIdentityOverlap(Span, Span),
    LeftIdentity(Span, Span),
    RightIdentity(Span, Span),
    LeftOverlap(Span, Span),
    RightOverlap(Span, Span),
    Disjoint(Span, Span, Span),
}

impl SpanCoalesced {
    fn new(left_res: MergeResult, right_res: MergeResult) -> Self {
        match (left_res, right_res) {
            (MergeResult::Merged(_), MergeResult::Merged(span)) => Self::Overlap(span),
            (MergeResult::Identity(span), MergeResult::Identity(_)) => Self::IdentityOverlap(span),
            (MergeResult::Identity(left_span), MergeResult::Merged(right_span)) => {
                Self::LeftIdentityOverlap(left_span, right_span)
            }
            (MergeResult::Merged(left_span), MergeResult::Identity(right_span)) => {
                Self::RightIdentityOverlap(left_span, right_span)
            }
            (MergeResult::Identity(left_span), MergeResult::UnMerged(_, right_span)) => {
                Self::LeftIdentity(left_span, right_span)
            }
            (MergeResult::UnMerged(left_span, _), MergeResult::Identity(right_span)) => {
                Self::RightIdentity(left_span, right_span)
            }
            (MergeResult::Merged(left_span), MergeResult::UnMerged(_, right_span)) => {
                Self::LeftOverlap(left_span, right_span)
            }
            (MergeResult::UnMerged(left_span, _), MergeResult::Merged(right_span)) => {
                Self::RightOverlap(left_span, right_span)
            }
            (MergeResult::UnMerged(left_span, orig), MergeResult::UnMerged(_, right_span)) => {
                Self::Disjoint(left_span, orig, right_span)
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Span {
    start: u64,
    end: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum MergeResult {
    Merged(Span),
    Identity(Span),
    UnMerged(Span, Span),
}

impl Span {
    fn new(start: u64, end: u64) -> Result<Span, SpanError> {
        if start >= end {
            Err(SpanError::InvalidSpan)
        } else {
            Ok(Span { start, end })
        }
    }

    fn merge(self, right: Span) -> MergeResult {
        match self.merge_right(right) {
            MergeResult::UnMerged(left, right) => match right.merge_right(left) {
                MergeResult::Merged(merged) => MergeResult::Merged(merged),
                _ => MergeResult::UnMerged(left, right),
            },
            res => res,
        }
    }

    fn merge_right(self, right: Span) -> MergeResult {
        if self == right {
            MergeResult::Identity(self)
        } else {
            if self.start <= right.start && self.end + 1 >= right.start {
                // Safe to unwrap, Span::new ensures that self.end > self.start
                // and other.start > other.end
                MergeResult::Merged(
                    Span::new(
                        std::cmp::min(self.start, right.start),
                        std::cmp::max(self.end, right.end),
                    )
                    .unwrap(),
                )
            } else {
                MergeResult::UnMerged(self, right)
            }
        }
    }

    pub fn coalesce(self, left_other: Span, right_other: Span) -> SpanCoalesced {
        let (left_merge, right_merge) = {
            let left_merge = left_other.merge_right(self);

            match left_merge {
                MergeResult::Identity(merged) | MergeResult::Merged(merged) => {
                    (left_merge, merged.merge_right(right_other))
                }
                _ => (left_merge, self.merge_right(right_other)),
            }
        };
        let ret = SpanCoalesced::new(left_merge, right_merge);
        ret
    }
}

#[derive(Clone, Debug, PartialEq)]
struct SpanVec {
    spans: Vec<Span>,
}

impl SpanVec {
    fn new() -> Self {
        SpanVec { spans: Vec::new() }
    }

    fn len(&self) -> usize {
        self.spans.len()
    }

    fn insert(&mut self, mut elem: Span) {
        // left most idx of partition window either triple or pair
        #[derive(Debug)]
        enum Window {
            Pair(usize),
            Triple(usize),
        }

        fn partition_to_window(len: usize, idx: usize) -> Option<Window> {
            match (idx, len) {
                // There's vec is empty, no window available
                (_, len) if len == 0 => None,
                // We're at the start of the vec
                (idx, _) if idx == 0 => Some(Window::Pair(idx)),
                // We're at the end of the vec
                (idx, len) if idx == len => Some(Window::Pair(idx - 1)),
                (idx, _) => Some(Window::Triple(idx - 1)),
            }
        }

        // Utility funciton to actuall merge in changes
        // Returns Some((rightmost_changed_idx, Span)) when the merge results in a modification
        fn merge_in_span(
            spans: &mut Vec<Span>,
            elem: Span,
            insert_idx: usize,
        ) -> Option<(usize, Span)> {
            if let Some(window) = partition_to_window(spans.len(), insert_idx) {
                match window {
                    // It's a short list or we're at the end
                    Window::Pair(l_idx) => {
                        let left_elem = spans[l_idx];
                        match left_elem.merge(elem) {
                            MergeResult::Identity(_) => {}
                            MergeResult::Merged(new_span) => {
                                spans[l_idx] = new_span;
                            }
                            MergeResult::UnMerged(_, _) => {
                                spans.insert(insert_idx, elem);
                            }
                        }
                        None
                    }
                    Window::Triple(l_idx) => {
                        // Get the existing spans, safe to unwrap as Window Triple requires len > 2
                        let [left, right] = <[Span; 2]>::try_from(&spans[l_idx..=l_idx + 1])
                            .ok()
                            .unwrap();
                        match elem.coalesce(left, right) {
                            // Overlap(Span),
                            SpanCoalesced::Overlap(new_span) => {
                                // Remove the right Span
                                spans.remove(l_idx + 1);
                                // Replace span under cursor with coalesced
                                spans[l_idx] = new_span;
                                Some((l_idx + 1, new_span))
                            }
                            // LeftOverlap(Span, Span),
                            SpanCoalesced::LeftOverlap(l_span, _) => {
                                spans[l_idx] = l_span;
                                Some((l_idx, l_span))
                            }
                            // RightOverlap(Span, Span),
                            SpanCoalesced::RightOverlap(_, new_span) => {
                                spans[l_idx + 1] = new_span;
                                Some((l_idx + 1, new_span))
                            }
                            // Disjoint(Span, Span, Span)
                            SpanCoalesced::Disjoint(_, elem, _) => {
                                spans.insert(l_idx + 1, elem);
                                None
                            }
                            // IdentityOverlap(Span),
                            SpanCoalesced::IdentityOverlap(elem) => {
                                // We've got duplicates in the vec, somehow...
                                spans.remove(l_idx + 1);
                                spans.remove(l_idx + 2);
                                Some((l_idx + 2, elem))
                            }
                            // LeftIdentityOverlap(Span, Span),
                            SpanCoalesced::LeftIdentityOverlap(_, new_span) => {
                                // We've got an unmerged overlap
                                // Overwrite the current index and cleanup the next elem
                                spans[l_idx] = new_span;
                                spans.remove(l_idx + 1);
                                Some((l_idx + 1, new_span))
                            }
                            // RightIdentityOverlap(Span, Span),
                            SpanCoalesced::RightIdentityOverlap(new_span, _) => {
                                // We've got an unmerged overlap
                                // Overwrite the current index and cleanup the next elem
                                spans[l_idx] = new_span;
                                spans.remove(l_idx + 1);
                                Some((l_idx + 1, new_span))
                            }
                            // LeftIdentity(Span, Span), RightIdentity(Span, Span)
                            SpanCoalesced::LeftIdentity(_, _)
                            | SpanCoalesced::RightIdentity(_, _) => {
                                // We're the same as the do nothing
                                None
                            }
                        }
                    }
                }
            } else {
                spans.insert(insert_idx, elem);
                None
            }
        }

        let elem_start = elem.start;
        // The location the span would be inserted in the the span
        let mut insert_idx = self.spans.partition_point(|&x| elem_start > x.start);

        while let Some((merge_idx, merge_elem)) = merge_in_span(&mut self.spans, elem, insert_idx) {
            insert_idx = merge_idx;
            elem = merge_elem;
        }
    }
}

impl std::ops::Index<usize> for SpanVec {
    type Output = Span;

    fn index(&self, idx: usize) -> &Self::Output {
        &self.spans[idx]
    }
}

#[test]
fn test_span_merge_right() {
    // Overlapping
    assert_eq!(
        Span::new(0, 1)
            .unwrap()
            .merge_right(Span::new(0, 2).unwrap()),
        MergeResult::Merged(Span::new(0, 2).unwrap())
    );
    // Adjacent
    assert_eq!(
        Span::new(0, 1)
            .unwrap()
            .merge_right(Span::new(2, 3).unwrap()),
        MergeResult::Merged(Span::new(0, 3).unwrap())
    );
    // Disjoint
    assert_eq!(
        Span::new(0, 1)
            .unwrap()
            .merge_right(Span::new(3, 4).unwrap()),
        MergeResult::UnMerged(Span::new(0, 1).unwrap(), Span::new(3, 4).unwrap())
    );
}

#[test]
fn test_span_merge() {
    // Overlapping
    assert_eq!(
        Span::new(0, 2).unwrap().merge(Span::new(0, 1).unwrap()),
        MergeResult::Merged(Span::new(0, 2).unwrap())
    );
    // Adjacent
    assert_eq!(
        Span::new(2, 3).unwrap().merge(Span::new(0, 1).unwrap()),
        MergeResult::Merged(Span::new(0, 3).unwrap())
    );
    // Disjoint
    assert_eq!(
        Span::new(0, 1).unwrap().merge(Span::new(3, 4).unwrap()),
        MergeResult::UnMerged(Span::new(0, 1).unwrap(), Span::new(3, 4).unwrap())
    );
}

#[test]
fn test_span_coalesce() {
    // Overlapping
    assert_eq!(
        Span::new(1, 2)
            .unwrap()
            .coalesce(Span::new(0, 1).unwrap(), Span::new(2, 3).unwrap()),
        SpanCoalesced::Overlap(Span::new(0, 3).unwrap())
    );
    // LeftOverlap
    assert_eq!(
        Span::new(1, 2)
            .unwrap()
            .coalesce(Span::new(0, 1).unwrap(), Span::new(4, 5).unwrap()),
        SpanCoalesced::LeftOverlap(Span::new(0, 2).unwrap(), Span::new(4, 5).unwrap())
    );
    // RightOverlap
    assert_eq!(
        Span::new(3, 4)
            .unwrap()
            .coalesce(Span::new(0, 1).unwrap(), Span::new(5, 9).unwrap()),
        SpanCoalesced::RightOverlap(Span::new(0, 1).unwrap(), Span::new(3, 9).unwrap())
    );
    // Disjoint
    assert_eq!(
        Span::new(3, 4)
            .unwrap()
            .coalesce(Span::new(0, 1).unwrap(), Span::new(6, 9).unwrap()),
        SpanCoalesced::Disjoint(
            Span::new(0, 1).unwrap(),
            Span::new(3, 4).unwrap(),
            Span::new(6, 9).unwrap()
        )
    );
}

#[test]
fn test_span_vec_insert_disjoint() {
    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_close = Span::new(0, 1).unwrap();

    sv.insert(s_far);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_far);

    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_close = Span::new(0, 1).unwrap();

    sv.insert(s_far);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_far);

    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_mid = Span::new(500, 550).unwrap();
    let s_close = Span::new(0, 1).unwrap();

    sv.insert(s_mid);
    sv.insert(s_far);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_mid);
    assert_eq!(sv[2], s_far);
}

#[test]
fn test_span_vec_insert_coalescing() {
    let mut sv = SpanVec::new();
    let s_close = Span::new(0, 1).unwrap();
    let s_mid = Span::new(1, 2).unwrap();
    let s_far = Span::new(3, 4).unwrap();

    sv.insert(s_far);
    assert_eq!(sv.len(), 1);
    sv.insert(s_mid);
    assert_eq!(sv.len(), 1);
    sv.insert(s_close);
    assert_eq!(sv.len(), 1);
    assert_eq!(sv[0], Span::new(0, 4).unwrap());

    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_close = Span::new(0, 1).unwrap();
    let s_big = Span::new(2, 999).unwrap();

    sv.insert(s_far);
    assert_eq!(sv.len(), 1);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_far);
    assert_eq!(sv.len(), 2);
    sv.insert(s_big);
    assert_eq!(sv[0], Span::new(0, 1100).unwrap());
    assert_eq!(sv.len(), 1);

    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_mid = Span::new(500, 550).unwrap();
    let s_bridge_mid_far = Span::new(551, 999).unwrap();
    let s_close = Span::new(0, 1).unwrap();

    sv.insert(s_mid);
    sv.insert(s_far);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_mid);
    assert_eq!(sv[2], s_far);
    sv.insert(s_bridge_mid_far);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], Span::new(500, 1100).unwrap());
    assert_eq!(sv.len(), 2);

    let mut sv = SpanVec::new();
    sv.insert(Span::new(0, 1).unwrap());
    sv.insert(Span::new(3, 4).unwrap());
    sv.insert(Span::new(6, 8).unwrap());
    sv.insert(Span::new(10, 11).unwrap());
    sv.insert(Span::new(13, 14).unwrap());
    sv.insert(Span::new(16, 17).unwrap());
    sv.insert(Span::new(19, 20).unwrap());
    sv.insert(Span::new(23, 24).unwrap());
    sv.insert(Span::new(26, 27).unwrap());
    sv.insert(Span::new(29, 30).unwrap());
    sv.insert(Span::new(400, 405).unwrap());

    assert_eq!(sv.len(), 11);

    sv.insert(Span::new(7, 399).unwrap());

    assert_eq!(sv.len(), 3, "{:#?}", sv);

    let mut sv = SpanVec::new();
    sv.insert(Span::new(0, 1).unwrap());
    sv.insert(Span::new(0, 1).unwrap());
    sv.insert(Span::new(3, 4).unwrap());
    sv.insert(Span::new(16, 17).unwrap());
    sv.insert(Span::new(19, 20).unwrap());
    sv.insert(Span::new(6, 8).unwrap());
    sv.insert(Span::new(23, 24).unwrap());
    sv.insert(Span::new(10, 11).unwrap());
    sv.insert(Span::new(26, 27).unwrap());
    sv.insert(Span::new(13, 14).unwrap());
    sv.insert(Span::new(29, 30).unwrap());
    sv.insert(Span::new(36, 37).unwrap());
    sv.insert(Span::new(34, 35).unwrap());
    sv.insert(Span::new(31, 32).unwrap());
    sv.insert(Span::new(33, 34).unwrap());
    sv.insert(Span::new(400, 405).unwrap());

    assert_eq!(sv.len(), 11);

    sv.insert(Span::new(9, 15).unwrap());
    assert_eq!(sv.len(), 8, "{:#?}", sv);

    sv.insert(Span::new(18, 35).unwrap());
    assert_eq!(sv.len(), 4, "{:#?}", sv);
}

#[derive(Clone)]
pub struct FileOffsetState<'a> {
    db: Arc<DB>,
    cf_opts: Options,
    rx: std::cell::RefCell<Option<async_channel::Receiver<FileOffsetEvent>>>,
    shutdown: std::cell::RefCell<Option<async_channel::Sender<FileOffsetEvent>>>,
    tx: async_channel::Sender<FileOffsetEvent>,
    file_offsets: Arc<async_lock::RwLock<HashMap<FileId, Vec<u64>>>>,
    pending_batch: Arc<async_lock::RwLock<SlotMap<DefaultKey, &'a [Offset]>>>,
}

impl<'a> FileOffsetState<'a> {
    fn new(db: Arc<DB>, cf_opts: Options) -> Self {
        let (tx, rx) = async_channel::unbounded();

        FileOffsetState {
            db,
            cf_opts,
            rx: std::cell::RefCell::new(Some(rx)),
            shutdown: std::cell::RefCell::new(Some(tx.clone())),
            tx,
            file_offsets: Arc::new(RwLock::new(HashMap::new())),
            pending_batch: Arc::new(RwLock::new(SlotMap::new())),
        }
    }

    pub fn offsets(&self) -> Result<Vec<FileOffset>, FileOffsetStateError> {
        let cf_handle = self.db.cf_handle(OFFSET_NAME).ok_or_else(|| {
            FileOffsetStateError::DbError("Failed to get ColumnFamily handle".into())
        })?;
        Ok(self
            .db
            .iterator_cf(cf_handle, IteratorMode::Start)
            .map(|(k, v)| {
                let (k_bytes, _) = k.split_at(std::mem::size_of::<u64>());
                let (v_bytes, _) = v.split_at(std::mem::size_of::<u64>());
                FileOffset {
                    key: FileId(u64::from_be_bytes(k_bytes.try_into().unwrap_or([0; 8]))),
                    offset: u64::from_be_bytes(v_bytes.try_into().unwrap_or([0; 8])),
                }
            })
            .collect::<Vec<_>>())
    }

    pub fn write_handle(&self) -> FileOffsetWriteHandle {
        FileOffsetWriteHandle {
            tx: self.tx.clone(),
        }
    }

    pub fn flush_handle(&self) -> FileOffsetFlushHandle {
        FileOffsetFlushHandle {
            tx: self.tx.clone(),
        }
    }

    pub fn shutdown_handle(&self) -> Result<FileOffsetShutdownHandle, FileOffsetStateError> {
        Ok(FileOffsetShutdownHandle {
            tx: self
                .shutdown
                .borrow_mut()
                .take()
                .ok_or(FileOffsetStateError::ShutdownHandleTaken)?,
        })
    }

    pub fn run(&self) -> Result<impl Future<Output = ()>, FileOffsetStateError> {
        let rx = self
            .rx
            .borrow_mut()
            .take()
            .ok_or(FileOffsetStateError::AlreadyRunning)?;
        let db = self.db.clone();
        Ok(rx
            .fold(Some(WriteBatch::default()), move |acc, event| {
                let db = db.clone();
                async move {
                    match db.cf_handle(OFFSET_NAME).ok_or_else(|| {
                        FileOffsetStateError::DbError("Failed to get ColumnFamily handle".into())
                    }) {
                        Ok(cf_handle) => match (acc, event) {
                            (Some(wb), FileOffsetEvent::Flush) => {
                                let ret = db.write(wb).map(|_| ());
                                ret.map(|_| None).map_err(|e| e.into())
                            }
                            (None, FileOffsetEvent::Flush) => Ok(None),
                            (wb, FileOffsetEvent::Update(e)) => {
                                let mut wb = wb.unwrap_or_default();
                                match e {
                                    FileOffsetUpdate::Update(FileOffset { key, offset }) => wb
                                        .put_cf(
                                            cf_handle,
                                            u64::to_be_bytes(key.0),
                                            u64::to_be_bytes(offset),
                                        ),
                                    FileOffsetUpdate::Delete(key) => {
                                        wb.delete_cf(cf_handle, u64::to_be_bytes(key.0))
                                    }
                                };
                                Ok(Some(wb))
                            }
                            (_, FileOffsetEvent::Clear) => Ok(None),
                        },
                        Err(e) => Err(e),
                    }
                    .map_err(|e| error!("{:?}", e))
                    .ok()
                    .flatten()
                }
            })
            .map(|_| ()))
    }
}

pub trait GetOffset {
    fn get_key(&self) -> Option<u64>;
    fn get_offset(&self) -> Option<u64>;
}

#[cfg(test)]
mod test {

    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn it_works() {
        let _ = env_logger::Builder::from_default_env().try_init();
        let data_dir = tempdir().expect("Could not create temp dir").into_path();

        // create a db, write to it, mutate it, delete entries.
        // The times/delays are significant
        fn _test(db_path: &std::path::Path, initial_count: usize) {
            let agent_state = AgentState::new(db_path).unwrap();
            let offset_state = agent_state.get_offset_state();

            let wh = offset_state.write_handle();
            let fh = offset_state.flush_handle();
            let sh = offset_state.shutdown_handle().unwrap();
            assert_eq!(initial_count, offset_state.offsets().unwrap().len());

            let paths = [1, 2, 3, 4];

            tokio_test::block_on(async {
                let _ = tokio::join!(
                    async {
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                        assert_eq!(4, offset_state.offsets().unwrap().len());
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                        assert_eq!(4, offset_state.offsets().unwrap().len());
                        assert_eq!(
                            13 * 2 + 14 * 2,
                            offset_state
                                .offsets()
                                .unwrap()
                                .iter()
                                .fold(0, |a, fo| a + fo.offset)
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                        assert_eq!(2, offset_state.offsets().unwrap().len());
                        sh.shutdown();
                    },
                    async move {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        for path in paths.iter() {
                            wh.update(&[(*path, 13)]).await.unwrap();
                        }
                        fh.flush().await.unwrap();
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                        for path in paths[..2].iter() {
                            wh.update(&[(*path, 14)]).await.unwrap();
                        }
                        fh.flush().await.unwrap();
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                        for path in paths[..2].iter() {
                            wh.delete(path).await.unwrap();
                        }
                        fh.flush().await.unwrap();
                    },
                    offset_state.run().unwrap()
                );
            });
        }
        _test(&data_dir, 0);
        _test(&data_dir, 2);
    }

    #[test]
    fn load_agent_state_dir_missing() {
        // build a path with multiple levels of missing directories to ensure they're all created
        let missing_state_dir = tempdir()
            .unwrap()
            .into_path()
            .join("a")
            .join("ghostly")
            .join("path");
        assert!(
            !missing_state_dir.exists(),
            "test prereq failed: {:?} reported as already existing",
            missing_state_dir
        );

        let result = AgentState::new(&missing_state_dir);
        assert!(result.is_ok());
        assert!(
            missing_state_dir.exists(),
            "state directory was not created"
        );
    }

    #[test]
    #[cfg(unix)]
    fn new_agent_state_dir_create_error() {
        use std::fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;

        let noperm_dir = tempdir().unwrap().into_path().join("denied");
        assert!(
            !noperm_dir.exists(),
            "test prereq failed: {:?} reported as already existing",
            noperm_dir
        );
        DirBuilder::new()
            .mode(0o000) // is only supported with the unix extension
            .create(&noperm_dir)
            .unwrap();

        assert!(
            noperm_dir.exists(),
            "test preqreq failed: failed to create {:?}",
            noperm_dir
        );
        assert!(
            noperm_dir.metadata().unwrap().permissions().readonly(),
            "test prereq failed: {:?} is not read only",
            noperm_dir
        );

        let result = AgentState::new(&noperm_dir).unwrap_err();
        assert!(matches!(result, StateError::PermissionDenied(p) if p == noperm_dir));
    }

    #[test]
    fn new_agent_state_not_dir() {
        let blocking_file = tempdir().unwrap().into_path().join("block");
        File::create(&blocking_file).unwrap();

        let result = AgentState::new(&blocking_file).unwrap_err();
        assert!(matches!(result, StateError::InvalidPath(p) if p == blocking_file));
    }

    #[test]
    fn new_agent_state_dir_exists() {
        let state_dir = tempdir().unwrap().into_path();
        let result = AgentState::new(&state_dir);
        assert!(result.is_ok(), "failed to create valid AgentState struct");
    }
}
