#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tera_hook::{Action, Hooks, Plugin};
use tera_protocol::{Object, Value};

struct State {
    enabled: bool,
    last_location: Option<Object>,
}

struct AntiAfk {
    started: Instant,
    state: Arc<Mutex<State>>,
}

impl AntiAfk {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            state: Arc::new(Mutex::new(State {
                enabled: true,
                last_location: None,
            })),
        }
    }
}

impl Plugin for AntiAfk {
    fn name(&self) -> &'static str {
        "anti-afk"
    }

    fn setup(&mut self, hooks: &mut Hooks) {
        let capture = Arc::clone(&self.state);
        hooks.on("C_PLAYER_LOCATION", 0, 100, move |event| {
            if let (Some(object), Ok(mut state)) = (event.object(), capture.lock()) {
                state.last_location = Some(object.clone());
            }
            Action::Pass
        });

        let started = self.started;
        let tick = Arc::clone(&self.state);
        hooks.every(Duration::from_secs(60), move |ticker| {
            let replay = tick.lock().ok().and_then(|state| {
                if state.enabled {
                    state.last_location.clone()
                } else {
                    None
                }
            });
            if let Some(location) = replay {
                let stamp = (started.elapsed().as_millis() as u64) & 0xffff_ffff;
                let location = location.with("time", Value::Uint(stamp));
                if ticker.send("C_PLAYER_LOCATION", &location) {
                    println!("[anti-afk] activite renvoyee");
                }
            }
        });

        let toggle = Arc::clone(&self.state);
        hooks.command("afk", move |command| {
            if let Ok(mut state) = toggle.lock() {
                state.enabled = match command.args.first().map(String::as_str) {
                    Some("on") => true,
                    Some("off") => false,
                    _ => !state.enabled,
                };
                let status = if state.enabled { "actif" } else { "coupe" };
                println!("[anti-afk] {status}");
                command.reply(&format!("anti-afk {status}"));
            }
        });
    }
}

tera_hook::export_mod!(AntiAfk::new());
