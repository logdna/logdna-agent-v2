use std::sync::Arc;

use http::types::body::LineBuilder;
use std::thread::spawn;

pub enum Status {
    Ok(LineBuilder),
    Skip,
}

pub trait Middleware: Send + Sync + 'static {
    fn run(&self);
    fn process(&self, lines: LineBuilder) -> Status;
}

#[derive(Default)]
pub struct Executor {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl Executor {
    pub fn new() -> Executor {
        Executor {
            middlewares: Vec::new(),
        }
    }

    pub fn register<T: Middleware>(&mut self, middleware: T) {
        self.middlewares.push(Arc::new(middleware))
    }

    pub fn init(&self) {
        for middleware in &self.middlewares {
            let middleware = middleware.clone();
            spawn(move || middleware.run());
        }
    }

    pub fn process(&self, line: LineBuilder) -> Option<LineBuilder> {
        self.middlewares
            .iter()
            .try_fold(line, |l, m| match m.process(l) {
                Status::Ok(l) => Ok(l),
                Status::Skip => Err(()),
            })
            .ok()
    }
}
