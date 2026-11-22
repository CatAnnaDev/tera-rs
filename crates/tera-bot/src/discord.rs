use std::sync::mpsc::{self, Sender};

use serde_json::json;

pub fn spawn(webhook: String) -> Sender<String> {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        while let Ok(content) = rx.recv() {
            let content = truncate(&content, 1900);
            let _ = ureq::post(&webhook)
                .send_json(json!({ "content": content, "allowed_mentions": { "parse": [] } }));
        }
    });
    tx
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}
