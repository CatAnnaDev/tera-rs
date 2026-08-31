use std::collections::HashMap;
use std::sync::Arc;
use tera_protocol::{value, Definition, Object, OpcodeMap, Registry};

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
        let Some(direction) = Direction::of(name) else {
            return false;
        };
        let (Some(opcode), Some(definition)) =
            (self.codec.code(name), self.codec.definition(name))
        else {
            return false;
        };
        match value::write(definition, opcode, object) {
            Ok(frame) => {
                self.send_raw(direction, frame);
                true
            }
            Err(_) => false,
        }
    }

    pub fn send_raw(&mut self, direction: Direction, frame: Vec<u8>) {
        self.injections.push(Injection { direction, frame });
    }
}

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
}

pub struct Engine {
    pub client_to_server: HashMap<u16, Vec<Handler>>,
    pub server_to_client: HashMap<u16, Vec<Handler>>,
}

impl Engine {
    pub fn build(mut plugins: Vec<Box<dyn Plugin>>, codec: &Codec) -> Self {
        let mut registrations = Vec::new();
        for plugin in plugins.iter_mut() {
            let name = plugin.name();
            let mut hooks = Hooks {
                codec,
                plugin: name,
                registrations: Vec::new(),
            };
            plugin.setup(&mut hooks);
            registrations.extend(hooks.registrations);
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
        Self {
            client_to_server,
            server_to_client,
        }
    }

    pub fn split(self) -> (HashMap<u16, Vec<Handler>>, HashMap<u16, Vec<Handler>>) {
        (self.client_to_server, self.server_to_client)
    }
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

pub const TERA_MOD_ABI: u32 = 1;

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
