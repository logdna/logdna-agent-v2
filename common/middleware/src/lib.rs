use std::sync::Arc;

use http::types::body::LineBuilder;
use std::thread::spawn;

pub enum Status {
    Ok(LineBuilder),
    Skip(LineBuilder),
}

pub trait Middleware: Send + Sync + 'static {
    fn run(&self);
    fn process(&self, line: LineBuilder) -> Status;
}

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

    pub fn process(&self, mut line: LineBuilder) -> Option<LineBuilder> {
            let mut skipped = false;

            for middleware in &self.middlewares {
                match middleware.process(line) {
                    Status::Ok(v) => {
                        line = v;
                    }
                    Status::Skip(v) => {
                        line = v;
                        skipped = true;
                        break;
                    }
                }
            }

            match skipped {
                true => None,
                false => Some(line),
            }
    }
}
