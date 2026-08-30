use crate::hooks::{Action, Event, Hooks, Plugin};
use std::sync::{Arc, Mutex};
use tera_protocol::{Object, Value};

pub struct ChatLogger;

impl Plugin for ChatLogger {
    fn name(&self) -> &'static str {
        "chat-logger"
    }

    fn setup(&mut self, hooks: &mut Hooks) {
        hooks.on("S_CHAT", 0, 100, log_chat);
        hooks.on("S_WHISPER", 0, 100, log_chat);
        hooks.on("S_PRIVATE_CHAT", 0, 100, log_chat);
    }
}

fn log_chat(event: &mut Event) -> Action {
    if let Some(object) = event.object() {
        let author = object.get("name").and_then(Value::as_str).unwrap_or("?");
        let message = object.get("message").and_then(Value::as_str).unwrap_or("");
        println!("  [chat/{}] {author}: {}", event.name, strip_markup(message));
    }
    Action::Pass
}

#[derive(Default)]
struct Player {
    name: String,
    game_id: u64,
    player_id: u32,
}

pub struct GameState {
    player: Arc<Mutex<Player>>,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            player: Arc::new(Mutex::new(Player::default())),
        }
    }
}

impl Plugin for GameState {
    fn name(&self) -> &'static str {
        "game-state"
    }

    fn setup(&mut self, hooks: &mut Hooks) {
        let on_login = Arc::clone(&self.player);
        hooks.on("S_LOGIN", 0, 90, move |event| {
            if let (Some(object), Ok(mut player)) = (event.object(), on_login.lock()) {
                if let Some(name) = object.get("name").and_then(Value::as_str) {
                    player.name = name.to_string();
                }
                if let Some(game_id) = object.get("gameId").and_then(Value::as_uint) {
                    player.game_id = game_id;
                }
                if let Some(player_id) = object.get("playerId").and_then(Value::as_uint) {
                    player.player_id = player_id as u32;
                }
            }
            Action::Pass
        });

        let on_spawn = Arc::clone(&self.player);
        hooks.on("S_SPAWN_ME", 0, 90, move |_event| {
            if let Ok(player) = on_spawn.lock() {
                println!(
                    "[game-state] {} spawned (gameId {}, playerId {})",
                    player.name, player.game_id, player.player_id
                );
            }
            Action::Pass
        });
    }
}

pub struct Example {
    pub drop: Vec<String>,
    pub retag: Vec<String>,
    pub announce: Vec<String>,
}

impl Plugin for Example {
    fn name(&self) -> &'static str {
        "example"
    }

    fn setup(&mut self, hooks: &mut Hooks) {
        for opcode in &self.drop {
            hooks.on(opcode, 0, 50, |_event| Action::Drop);
        }
        for opcode in &self.retag {
            hooks.on(opcode, 0, 50, retag_message);
        }
        for opcode in &self.announce {
            hooks.on(opcode, 0, 50, announce);
        }
    }
}

fn retag_message(event: &mut Event) -> Action {
    let Some(object) = event.object_mut() else {
        return Action::Pass;
    };
    let Some(message) = object.get("message").and_then(Value::as_str).map(str::to_string) else {
        return Action::Pass;
    };
    object.set("message", Value::Str(format!("[proxy] {message}")));
    Action::Modify
}

fn announce(event: &mut Event) -> Action {
    let mut message = Object::new();
    message.set("channel", Value::Uint(0));
    message.set("name", Value::Str("proxy".to_string()));
    message.set("message", Value::Str("injected by tera-proxy".to_string()));
    event.send("S_CHAT", &message);
    Action::Pass
}

fn strip_markup(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut in_tag = false;
    for character in message.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(character),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

pub fn builtin() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(ChatLogger),
        Box::new(GameState::new()),
        Box::new(Example {
            drop: Vec::new(),
            retag: Vec::new(),
            announce: Vec::new(),
        }),
    ]
}
