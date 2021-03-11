use std::sync::Arc;

use http::types::body::LineMetaMut;
use std::thread::spawn;

pub enum Status<T> {
    Ok(T),
    Skip,
}

pub trait Middleware: Send + Sync + 'static {
    fn run(&self);
    fn process<'a>(&self, lines: &'a mut dyn LineMetaMut) -> Status<&'a mut dyn LineMetaMut>;
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

    pub fn process<'a>(&self, line: &'a mut dyn LineMetaMut) -> Option<&'a mut dyn LineMetaMut> {
        self.middlewares
            .iter()
            .try_fold(line, |l, m| match m.process(l) {
                Status::Ok(l) => Ok(l),
                Status::Skip => Err(()),
            })
            .ok()
    }
}
