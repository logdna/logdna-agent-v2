use http::types::body::{LineBufferMut, LineBuilder};
use std::sync::Arc;
use std::{ops::Deref, thread::spawn};

pub mod k8s_line_rules;
pub mod line_rules;
pub mod meta_rules;

pub enum Status<T> {
    Ok(T),
    Skip,
    Retry,
}

pub trait Middleware: Send + Sync + 'static {
    fn run(&self);
    fn process<'a>(&self, lines: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut>;
}

impl<T, U> Middleware for T
where
    T: Deref<Target = U> + Send + Sync + 'static,
    U: Middleware,
{
    fn run(&self) {
        self.deref().run()
    }

    fn process<'a>(&self, lines: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut> {
        self.deref().process(lines)
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

pub struct Executor {
    middlewares: Vec<Arc<dyn Middleware>>,
    retry_lines_send: async_channel::Sender<LineBuilder>,
}

impl Executor {
    pub fn new(retry_lines_send: async_channel::Sender<LineBuilder>) -> Executor {
        Executor {
            middlewares: Vec::new(),
            retry_lines_send,
        }
    }

    pub fn register<T: Middleware>(&mut self, middleware: T) {
        self.middlewares
            .push(ArcMiddleware::from(middleware).into_inner())
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
                Status::Retry => Err(()),
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
