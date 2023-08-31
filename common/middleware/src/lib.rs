use http::types::body::LineBufferMut;
use std::sync::Arc;
use std::{ops::Deref, thread::spawn};
use thiserror::Error;
use tracing::info;

pub mod k8s_line_rules;
pub mod line_rules;
pub mod meta_rules;

pub enum Status<T> {
    Ok(T),
    Skip,
}
#[derive(Debug, Error)]
pub enum MiddlewareError {
    #[error("line needs to be dropped")]
    Skip,
    #[error("line needs to be retried for processing again later")]
    Retry,
}

pub trait Middleware: Send + Sync + 'static {
    fn run(&self);
    fn process<'a>(&self, line: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut>;
    fn validate<'a>(
        &self,
        line: &'a dyn LineBufferMut,
    ) -> Result<&'a dyn LineBufferMut, MiddlewareError>;
    fn name(&self) -> &'static str;
}

impl<T, U> Middleware for T
where
    T: Deref<Target = U> + Send + Sync + 'static,
    U: Middleware,
{
    fn run(&self) {
        self.deref().run()
    }

    fn process<'a>(&self, line: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut> {
        self.deref().process(line)
    }

    fn validate<'a>(
        &self,
        line: &'a dyn LineBufferMut,
    ) -> Result<&'a dyn LineBufferMut, MiddlewareError> {
        Ok(line)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}

pub struct ArcMiddleware<T>(Arc<T>);

impl<T> ArcMiddleware<T> {
    fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T> From<Arc<T>> for ArcMiddleware<T>
where
    T: Middleware,
{
    fn from(middleware: Arc<T>) -> ArcMiddleware<T> {
        Self(middleware)
    }
}

impl<T> From<T> for ArcMiddleware<T>
where
    T: Middleware,
{
    fn from(middleware: T) -> ArcMiddleware<T> {
        Self(Arc::new(middleware))
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

    pub fn register<T: Middleware>(&mut self, middleware: T) {
        info!("register middleware: {}", middleware.name());
        self.middlewares
            .push(ArcMiddleware::from(middleware).into_inner());
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
    ) -> Result<&'a mut dyn LineBufferMut, MiddlewareError> {
        self.middlewares
            .iter()
            .try_fold(line, |l, m| match m.process(l) {
                Status::Ok(l) => Ok(l),
                Status::Skip => Err(MiddlewareError::Skip),
            })
    }

    pub fn validate<'a>(
        &self,
        line: &'a dyn LineBufferMut,
    ) -> Result<&'a dyn LineBufferMut, MiddlewareError> {
        self.middlewares
            .iter()
            .try_fold(line, |l, m| match m.validate(l) {
                Ok(_) => Ok(l),
                Err(e) => Err(e),
            })
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
