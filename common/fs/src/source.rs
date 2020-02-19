use crate::rule::Rules;
use crate::tail::Tailer;
use crate::watch::Watcher;

use std::path::PathBuf;

use http::types::body::LineBuilder;

use source::Source;

pub struct FSSource {
    watcher: Watcher,
    tailer: Tailer,
}
impl FSSource {
    pub fn new<T: AsRef<[PathBuf]>, R: Into<Rules>>(path: T, rules: R) -> FSSource {
        let mut watcher = Watcher::builder()
            .add_all(path)
            .append_all(rules)
            .build()
            .unwrap();
        watcher.init();

        FSSource {
            watcher,
            tailer: Tailer::new(),
        }
    }
}

impl<'a> Source<'a> for FSSource {
    fn drain(&mut self, callback: &mut Box<dyn FnMut(LineBuilder) + 'a>) {
        let watcher = &mut self.watcher;
        let tailer = &mut self.tailer;
        watcher.read_events(|event| {
            tailer.process(event, |line| callback(line));
        });
    }
}
