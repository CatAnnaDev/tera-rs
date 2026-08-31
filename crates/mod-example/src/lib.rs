#![forbid(unsafe_code)]

use tera_hook::{Action, Hooks, Plugin};
use tera_protocol::Value;

struct Example;

impl Plugin for Example {
    fn name(&self) -> &'static str {
        "example"
    }

    fn setup(&mut self, hooks: &mut Hooks) {
        hooks.on("S_CHAT", 0, 100, |event| {
            if let Some(object) = event.object() {
                let author = object.get("name").and_then(Value::as_str).unwrap_or("");
                let message = object.get("message").and_then(Value::as_str).unwrap_or("");
                println!("[mod-example] {author}: {message}");
            }
            Action::Pass
        });
        hooks.on("S_LOGIN", 0, 100, |event| {
            if let Some(object) = event.object() {
                if let Some(name) = object.get("name").and_then(Value::as_str) {
                    println!("[mod-example] login: {name}");
                }
            }
            Action::Pass
        });
    }
}

tera_hook::export_mod!(Example);
