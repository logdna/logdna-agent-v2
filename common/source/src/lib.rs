use http::types::body::LineBuilder;

pub trait Source<'a> {
    fn drain(&mut self, callback: &mut Box<dyn FnMut(Vec<LineBuilder>) + 'a>);
}

pub struct SourceReader<'a> {
    sources: Vec<Box<dyn Source<'a>>>,
}

impl<'a> SourceReader<'a> {
    pub fn new() -> SourceReader<'a> {
        SourceReader {
            sources: Vec::new(),
        }
    }

    pub fn register<T: Source<'a> + 'static>(&mut self, source: T) {
        self.sources.push(Box::new(source))
    }

    pub fn drain(&mut self, mut callback: Box<dyn FnMut(Vec<LineBuilder>) + 'a>) {
        for source in &mut self.sources {
            source.drain(&mut callback);
        }
    }
}
