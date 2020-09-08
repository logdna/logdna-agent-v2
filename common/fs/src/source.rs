use crate::rule::Rules;
use crate::tail::Tailer;

use std::path::PathBuf;

use http::types::body::LineBuilder;

use source::Source;

pub struct FSSource {
    tailer: Tailer,
}
impl FSSource {
    pub fn new<R: Into<Rules>>(paths: Vec<PathBuf>, rules: R) -> FSSource {
        FSSource {
            tailer: Tailer::new(paths, rules.into()),
        }
    }
}

impl<'a> Source<'a> for FSSource {
    fn drain(&mut self, mut callback: &mut (dyn FnMut(Vec<LineBuilder>) + 'a)) {
        let tailer = &mut self.tailer;
        tailer.process(&mut callback);
    }
}
