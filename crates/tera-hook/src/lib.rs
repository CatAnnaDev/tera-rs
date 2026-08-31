use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tera_protocol::{value, Definition, Object, OpcodeMap, Registry, Value};

pub struct Codec {
    opcodes: OpcodeMap,
    registry: Registry,
}

impl Codec {
    pub fn new(opcodes: OpcodeMap, registry: Registry) -> Arc<Self> {
        Arc::new(Self { opcodes, registry })
    }

    pub fn code(&self, name: &str) -> Option<u16> {
        self.opcodes.code(name)
    }

    pub fn name(&self, code: u16) -> Option<&str> {
        self.opcodes.name(code)
    }

    pub fn version(&self, name: &str) -> Option<u32> {
        self.registry.version(name)
    }

    pub fn definition(&self, name: &str) -> Option<&Definition> {
        self.registry.get(name)
    }

    pub fn decode(&self, name: &str, frame: &[u8]) -> Option<Object> {
        self.definition(name)
            .and_then(|definition| value::read(definition, frame).ok())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}

impl Direction {
    pub fn of(name: &str) -> Option<Self> {
        if name.starts_with("C_") {
            Some(Self::ClientToServer)
        } else if name.starts_with("S_") || name.starts_with("I_") {
            Some(Self::ServerToClient)
        } else {
            None
        }
    }
}

pub enum Action {
    Pass,
    Modify,
    Drop,
}

pub struct Injection {
    pub direction: Direction,
    pub frame: Vec<u8>,
}

fn build_frame(codec: &Codec, name: &str, object: &Object) -> Option<(Direction, Vec<u8>)> {
    let direction = Direction::of(name)?;
    let opcode = codec.code(name)?;
    let definition = codec.definition(name)?;
    let frame = value::write(definition, opcode, object).ok()?;
    Some((direction, frame))
}

pub struct Event<'a> {
    pub name: &'a str,
    object: Option<&'a mut Object>,
    codec: &'a Codec,
    injections: &'a mut Vec<Injection>,
}

impl Event<'_> {
    pub fn object(&self) -> Option<&Object> {
        self.object.as_deref()
    }

    pub fn object_mut(&mut self) -> Option<&mut Object> {
        self.object.as_deref_mut()
    }

    pub fn send(&mut self, name: &str, object: &Object) -> bool {
        match build_frame(self.codec, name, object) {
            Some((direction, frame)) => {
                self.send_raw(direction, frame);
                true
            }
            None => false,
        }
    }

    pub fn send_raw(&mut self, direction: Direction, frame: Vec<u8>) {
        self.injections.push(Injection { direction, frame });
    }
}

pub struct Ticker<'a> {
    codec: &'a Codec,
    injections: &'a mut Vec<Injection>,
}

impl Ticker<'_> {
    pub fn send(&mut self, name: &str, object: &Object) -> bool {
        match build_frame(self.codec, name, object) {
            Some((direction, frame)) => {
                self.send_raw(direction, frame);
                true
            }
            None => false,
        }
    }

    pub fn send_raw(&mut self, direction: Direction, frame: Vec<u8>) {
        self.injections.push(Injection { direction, frame });
    }
}

pub type TimerHandler = Box<dyn FnMut(&mut Ticker) + Send>;

pub struct Timer {
    pub interval: Duration,
    pub repeat: bool,
    handler: TimerHandler,
}

impl Timer {
    pub fn fire(&mut self, codec: &Codec) -> Vec<Injection> {
        let mut injections = Vec::new();
        let mut ticker = Ticker {
            codec,
            injections: &mut injections,
        };
        (self.handler)(&mut ticker);
        injections
    }
}

const PROXY_CHANNEL_SLOT: i64 = 7;
const PROXY_CHANNEL_CHAT: u64 = 18;
const PROXY_CHANNEL_ID: i64 = 31337;

pub struct Command<'a, 'b> {
    pub args: Vec<String>,
    event: &'a mut Event<'b>,
}

impl Command<'_, '_> {
    pub fn reply(&mut self, text: &str) {
        let object = Object::new()
            .with("channel", Value::Uint(PROXY_CHANNEL_ID as u64))
            .with("authorID", Value::Uint(0))
            .with("authorName", Value::Str("proxy".to_string()))
            .with("message", Value::Str(text.to_string()));
        self.event.send("S_PRIVATE_CHAT", &object);
    }

    pub fn send(&mut self, name: &str, object: &Object) -> bool {
        self.event.send(name, object)
    }
}

pub type CommandHandler = Box<dyn FnMut(&mut Command) + Send>;

pub enum Outcome {
    Pass,
    Modify(Object),
    Drop,
}

pub type Handler = Box<dyn FnMut(&mut Event) -> Action + Send>;

pub trait Plugin: Send {
    fn name(&self) -> &'static str;
    fn setup(&mut self, hooks: &mut Hooks);
}

struct Registration {
    name: String,
    direction: Direction,
    order: i32,
    handler: Handler,
}

pub struct Hooks<'a> {
    codec: &'a Codec,
    plugin: &'static str,
    registrations: Vec<Registration>,
    timers: Vec<Timer>,
    commands: Vec<(String, CommandHandler)>,
}

impl Hooks<'_> {
    pub fn on(
        &mut self,
        name: &str,
        version: u32,
        order: i32,
        handler: impl FnMut(&mut Event) -> Action + Send + 'static,
    ) {
        match Direction::of(name) {
            Some(direction) => self.on_at(name, direction, version, order, handler),
            None => eprintln!(
                "[hooks] {}: {name} has no C_/S_/I_ prefix, use on_at",
                self.plugin
            ),
        }
    }

    pub fn on_at(
        &mut self,
        name: &str,
        direction: Direction,
        version: u32,
        order: i32,
        handler: impl FnMut(&mut Event) -> Action + Send + 'static,
    ) {
        if self.codec.code(name).is_none() {
            eprintln!("[hooks] {}: unknown opcode {name}, hook ignored", self.plugin);
            return;
        }
        if version != 0 && self.codec.version(name) != Some(version) {
            eprintln!(
                "[hooks] {}: {name} v{version} requested, loaded {:?}",
                self.plugin,
                self.codec.version(name)
            );
        }
        self.registrations.push(Registration {
            name: name.to_string(),
            direction,
            order,
            handler: Box::new(handler),
        });
    }

    pub fn every(&mut self, interval: Duration, handler: impl FnMut(&mut Ticker) + Send + 'static) {
        self.timers.push(Timer {
            interval,
            repeat: true,
            handler: Box::new(handler),
        });
    }

    pub fn after(&mut self, delay: Duration, handler: impl FnMut(&mut Ticker) + Send + 'static) {
        self.timers.push(Timer {
            interval: delay,
            repeat: false,
            handler: Box::new(handler),
        });
    }

    pub fn command(&mut self, name: &str, handler: impl FnMut(&mut Command) + Send + 'static) {
        self.commands.push((name.to_string(), Box::new(handler)));
    }
}

pub struct Engine {
    pub client_to_server: HashMap<u16, Vec<Handler>>,
    pub server_to_client: HashMap<u16, Vec<Handler>>,
    pub timers: Vec<Timer>,
}

impl Engine {
    pub fn build(mut plugins: Vec<Box<dyn Plugin>>, codec: &Codec) -> Self {
        let mut registrations = Vec::new();
        let mut timers = Vec::new();
        let mut commands: Vec<(String, CommandHandler)> = Vec::new();
        for plugin in plugins.iter_mut() {
            let name = plugin.name();
            let mut hooks = Hooks {
                codec,
                plugin: name,
                registrations: Vec::new(),
                timers: Vec::new(),
                commands: Vec::new(),
            };
            plugin.setup(&mut hooks);
            registrations.extend(hooks.registrations);
            timers.extend(hooks.timers);
            commands.extend(hooks.commands);
        }
        registrations.sort_by_key(|registration| registration.order);

        let mut client_to_server: HashMap<u16, Vec<Handler>> = HashMap::new();
        let mut server_to_client: HashMap<u16, Vec<Handler>> = HashMap::new();
        for registration in registrations {
            let Some(code) = codec.code(&registration.name) else {
                continue;
            };
            let table = match registration.direction {
                Direction::ClientToServer => &mut client_to_server,
                Direction::ServerToClient => &mut server_to_client,
            };
            table.entry(code).or_default().push(registration.handler);
        }

        if !commands.is_empty() {
            match codec.code("C_CHAT") {
                Some(code) => client_to_server
                    .entry(code)
                    .or_default()
                    .insert(0, command_dispatcher(commands)),
                None => eprintln!("[hooks] commands registered but C_CHAT unknown, disabled"),
            }
            if let Some(code) = codec.code("S_LOGIN") {
                server_to_client
                    .entry(code)
                    .or_default()
                    .insert(0, channel_creator());
            }
            if let Some(code) = codec.code("C_REQUEST_PRIVATE_CHANNEL_INFO") {
                client_to_server
                    .entry(code)
                    .or_default()
                    .insert(0, channel_info());
            }
        }

        Self {
            client_to_server,
            server_to_client,
            timers,
        }
    }

    pub fn split(self) -> (HashMap<u16, Vec<Handler>>, HashMap<u16, Vec<Handler>>, Vec<Timer>) {
        (self.client_to_server, self.server_to_client, self.timers)
    }
}

fn command_dispatcher(mut commands: Vec<(String, CommandHandler)>) -> Handler {
    Box::new(move |event: &mut Event| {
        let channel = event
            .object()
            .and_then(|object| object.get("channel"))
            .and_then(Value::as_uint)
            .unwrap_or(0);
        if channel != PROXY_CHANNEL_CHAT {
            return Action::Pass;
        }
        let text = event
            .object()
            .and_then(|object| object.get("message"))
            .and_then(Value::as_str)
            .map(|message| message.to_string())
            .unwrap_or_default();
        let mut parts = text.split_whitespace();
        let name = parts.next().unwrap_or("").to_string();
        let args: Vec<String> = parts.map(|part| part.to_string()).collect();
        let target = commands.iter().position(|(command, _)| *command == name);
        let mut command = Command { args, event };
        match target {
            Some(index) => (commands[index].1)(&mut command),
            None => command.reply(&format!("commande inconnue: {name}")),
        }
        Action::Drop
    })
}

fn channel_creator() -> Handler {
    let mut created = false;
    Box::new(move |event: &mut Event| {
        if !created {
            created = true;
            let object = Object::new()
                .with("index", Value::Int(PROXY_CHANNEL_SLOT))
                .with("channelId", Value::Int(PROXY_CHANNEL_ID))
                .with("unk", Value::Array(Vec::new()))
                .with("name", Value::Str("proxy".to_string()));
            event.send("S_JOIN_PRIVATE_CHANNEL", &object);
        }
        Action::Pass
    })
}

fn channel_info() -> Handler {
    Box::new(move |event: &mut Event| {
        let requested = event
            .object()
            .and_then(|object| object.get("channelId"))
            .and_then(Value::as_int)
            .unwrap_or(0);
        if requested != PROXY_CHANNEL_ID {
            return Action::Pass;
        }
        let member = Object::new().with("name", Value::Str("proxy".to_string()));
        let object = Object::new()
            .with("owner", Value::Bool(false))
            .with("password", Value::Uint(0))
            .with("members", Value::Array(vec![member]))
            .with("friends", Value::Array(Vec::new()));
        event.send("S_REQUEST_PRIVATE_CHANNEL_INFO", &object);
        Action::Drop
    })
}

pub fn dispatch(
    table: &mut HashMap<u16, Vec<Handler>>,
    opcode: u16,
    name: &str,
    object: Option<Object>,
    injections: &mut Vec<Injection>,
    codec: &Codec,
) -> Outcome {
    let Some(handlers) = table.get_mut(&opcode) else {
        return Outcome::Pass;
    };
    let mut owned = object;
    let mut dropped = false;
    let mut modified = false;
    for handler in handlers.iter_mut() {
        let mut event = Event {
            name,
            object: owned.as_mut(),
            codec,
            injections: &mut *injections,
        };
        let action =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&mut event))) {
                Ok(action) => action,
                Err(_) => {
                    eprintln!("[hooks] {name}: handler panicked, packet passed through");
                    Action::Pass
                }
            };
        match action {
            Action::Drop => dropped = true,
            Action::Modify => modified = true,
            Action::Pass => {}
        }
    }
    if dropped {
        Outcome::Drop
    } else if modified {
        match owned {
            Some(object) => Outcome::Modify(object),
            None => Outcome::Pass,
        }
    } else {
        Outcome::Pass
    }
}

#[derive(Default)]
pub struct Stats {
    by_name: HashMap<String, (u64, u64)>,
}

impl Stats {
    pub fn record(&mut self, name: &str, len: usize) {
        if let Some(entry) = self.by_name.get_mut(name) {
            entry.0 += 1;
            entry.1 += len as u64;
        } else {
            self.by_name.insert(name.to_string(), (1, len as u64));
        }
    }

    pub fn dump(&self) {
        let mut rows: Vec<_> = self.by_name.iter().collect();
        rows.sort_by(|left, right| right.1 .1.cmp(&left.1 .1));
        println!("=== traffic stats (by volume) ===");
        for (name, (count, bytes)) in rows {
            println!("  {count:>6} x {name:<34} {bytes:>10} b");
        }
    }
}

pub const TERA_MOD_ABI: u32 = 3;

pub struct ModRegistration {
    pub plugin: Box<dyn Plugin>,
}

#[macro_export]
macro_rules! export_mod {
    ($ctor:expr) => {
        #[no_mangle]
        pub extern "C" fn tera_mod_abi() -> u32 {
            $crate::TERA_MOD_ABI
        }

        #[no_mangle]
        pub extern "C" fn tera_mod_register() -> *mut $crate::ModRegistration {
            let plugin: ::std::boxed::Box<dyn $crate::Plugin> = ::std::boxed::Box::new($ctor);
            ::std::boxed::Box::into_raw(::std::boxed::Box::new($crate::ModRegistration { plugin }))
        }
    };
}
