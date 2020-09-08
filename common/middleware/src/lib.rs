use std::sync::Arc;

use http::types::body::LineBuilder;
use std::thread::spawn;

pub enum Status {
    Ok(Vec<LineBuilder>),
    Skip,
}

pub trait Middleware: Send + Sync + 'static {
    fn run(&self);
    fn process(&self, lines: Vec<LineBuilder>) -> Status;
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

    pub fn process(&self, mut lines: Vec<LineBuilder>) -> Option<Vec<LineBuilder>> {
        let mut skipped = false;

        for middleware in &self.middlewares {
            match middleware.process(lines.clone()) {
                Status::Ok(v) => {
                    lines = v;
                }
                Status::Skip => {
                    skipped = true;
                    break;
                }
            }
        }

        if skipped {
            None
        } else {
            Some(lines)
        }
    }
}
