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

pub enum MixedMiddleware<T: Middleware> {
    DirectMiddleware(T),
    ArcMiddleware(Arc<T>),
}

impl<T> MixedMiddleware<T>
where
    T: Middleware,
{
    fn take(self) -> Arc<T> {
        match self {
            MixedMiddleware::DirectMiddleware(middleware) => Arc::new(middleware),
            MixedMiddleware::ArcMiddleware(middleware) => middleware,
        }
    }
}

impl<T> From<Arc<T>> for MixedMiddleware<T>
where
    T: Middleware,
{
    fn from(middleware: Arc<T>) -> MixedMiddleware<T> {
        MixedMiddleware::ArcMiddleware(middleware)
    }
}

impl<T> From<T> for MixedMiddleware<T>
where
    T: Middleware,
{
    fn from(middleware: T) -> MixedMiddleware<T> {
        MixedMiddleware::DirectMiddleware(middleware)
    }
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

    pub fn register<T: Into<MixedMiddleware<Z>>, Z: Middleware>(&mut self, mixed_middleware: T) {
        self.middlewares.push(mixed_middleware.into().take())
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

#[cfg(test)]
pub mod test {
    // Provide values for extern symbols PKG_NAME and PKG_VERSION
    // when building this module on it's own
    #[no_mangle]
    pub static PKG_NAME: &str = "test";
    #[no_mangle]
    pub static PKG_VERSION: &str = "test";
}
