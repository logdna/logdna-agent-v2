use std::sync::Arc;

use http::types::body::LineBufferMut;
use std::thread::spawn;

pub mod k8s_line_rules;
pub mod line_rules;
pub mod meta_rules;

pub enum Status<T> {
    Ok(T),
    Skip,
}

pub trait Middleware: Send + Sync + 'static {
    fn run(&self);
    fn process<'a>(&self, lines: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut>;
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

    pub fn process<'a>(
        &self,
        line: &'a mut dyn LineBufferMut,
    ) -> Option<&'a mut dyn LineBufferMut> {
        self.middlewares
            .iter()
            .try_fold(line, |l, m| match m.process(l) {
                Status::Ok(l) => Ok(l),
                Status::Skip => Err(()),
            })
            .ok()
    }
}
