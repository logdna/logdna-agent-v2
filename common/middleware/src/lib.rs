use std::sync::Arc;

use crossbeam::{bounded, Receiver, Sender};
use crossbeam::scope;

use http::types::body::LineBuilder;

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
    senders: Vec<Sender<LineBuilder>>,

    line_sender: Sender<LineBuilder>,
    line_receiver: Receiver<LineBuilder>,
}

impl Executor {
    pub fn new() -> Executor {
        let (s, r) = bounded(0);
        Executor {
            middlewares: Vec::new(),
            senders: Vec::new(),
            line_sender: s,
            line_receiver: r,
        }
    }

    pub fn register<T: Middleware>(&mut self, middleware: T) {
        self.middlewares.push(Arc::new(middleware))
    }

    pub fn add_sender(&mut self, sender: Sender<LineBuilder>) {
        self.senders.push(sender)
    }

    pub fn sender(&self) -> Sender<LineBuilder> {
        self.line_sender.clone()
    }

    pub fn run(self) {
        scope(|s| {
            s.spawn(|_| self.process());
            s.spawn(|s| {
                for middleware in &self.middlewares {
                    let middleware = middleware.clone();
                    s.spawn(move |_| middleware.run());
                }
            });
        }).expect("Executor::run()");
    }

    fn process(&self) {
        loop {
            let mut line = self.line_receiver.recv().unwrap();
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
            };

            if skipped {
                continue;
            }

            match self.senders.len() {
                0 => { self.senders.get(0).unwrap().send(line).unwrap() }
                _ => {
                    self.senders.iter().for_each(|s| s.send(line.clone()).unwrap())
                }
            }
        }
    }
}