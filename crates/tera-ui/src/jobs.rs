use std::sync::mpsc::{channel, Receiver, Sender};

pub struct Progress {
    pub label: String,
    pub done: usize,
    pub total: usize,
}

pub enum Message {
    Progress(Progress),
    Done(Result<String, String>),
}

pub struct Job {
    pub label: String,
    receiver: Receiver<Message>,
    pub done: usize,
    pub total: usize,
    pub detail: String,
}

impl Job {
    pub fn spawn<F>(label: impl Into<String>, work: F) -> Self
    where
        F: FnOnce(&Sender<Message>) -> Result<String, String> + Send + 'static,
    {
        let (sender, receiver) = channel();
        let label = label.into();
        let thread_sender = sender.clone();
        std::thread::spawn(move || {
            let outcome = work(&thread_sender);
            let _ = thread_sender.send(Message::Done(outcome));
        });
        Self {
            label,
            receiver,
            done: 0,
            total: 0,
            detail: String::new(),
        }
    }

    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let mut finished = None;
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                Message::Progress(progress) => {
                    self.done = progress.done;
                    self.total = progress.total;
                    self.detail = progress.label;
                }
                Message::Done(result) => finished = Some(result),
            }
        }
        finished
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }
}

pub fn report(sender: &Sender<Message>, label: impl Into<String>, done: usize, total: usize) {
    let _ = sender.send(Message::Progress(Progress {
        label: label.into(),
        done,
        total,
    }));
}
