use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use tera_protocol::handshake::MAGIC;
use tera_protocol::session::MODERN;
use tera_protocol::value::{write as write_packet, Object, Value};
use tera_protocol::{OpcodeMap, PacketBuffer, Session};
use tera_server::responses::Responses;
use tera_server::session::Server;
use tera_server::{log::Log, registry::Registry, session::serve, world::World};

fn data_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data"))
}

fn opcode_map() -> OpcodeMap {
    OpcodeMap::parse(
        "C_CHECK_VERSION 19900\nS_CHECK_VERSION 27259\nC_LOGIN_ARBITER 40075\nS_LOGIN_ARBITER 55074\n",
    )
    .unwrap()
}

#[test]
fn client_completes_the_handshake_and_gets_an_answer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let registry = Registry::load(&[data_root().join("definitions")], Some(100)).unwrap();
        let logger = Log::new(false);
        let _ = serve(
            stream,
            &Server {
                opcodes: &opcode_map(),
                registry: &registry,
                logger: &logger,
                world: &World::default(),
                worlds: &tera_server::worlds::Worlds::default(),
                npcs: &tera_server::npcs::Npcs::default(),
                items: &tera_server::items::Items::default(),
                skills: &tera_server::skills::Skills::default(),
                realm: &tera_server::realm::Realm::default(),
                spawns: &tera_server::spawns::Spawns::default(),
                responses: &Responses::default(),
                auto_reply: false,
                auto_reply_aliases: false,
            },
            false,
        );
    });

    let mut stream = TcpStream::connect(address).unwrap();
    let mut greeting = [0u8; 4];
    stream.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, MAGIC);

    let client_first = [0x11u8; 128];
    let client_second = [0x22u8; 128];
    stream.write_all(&client_first).unwrap();
    let mut server_first = [0u8; 128];
    stream.read_exact(&mut server_first).unwrap();
    stream.write_all(&client_second).unwrap();
    let mut server_second = [0u8; 128];
    stream.read_exact(&mut server_second).unwrap();

    let mut session = Session::new(
        &client_first,
        &client_second,
        &server_first,
        &server_second,
        MODERN,
    );

    let registry = Registry::load(&[data_root().join("definitions")], Some(100)).unwrap();
    let definition = registry.get("C_CHECK_VERSION").unwrap();
    let request = Object::new().with(
        "version",
        Value::Array(vec![
            Object::new()
                .with("index", Value::Int(0))
                .with("value", Value::Int(387463)),
            Object::new()
                .with("index", Value::Int(1))
                .with("value", Value::Int(387463)),
        ]),
    );
    let mut packet = write_packet(definition, 19900, &request).unwrap();
    session.decrypt(&mut packet);
    stream.write_all(&packet).unwrap();

    let mut buffer = [0u8; 512];
    let read = stream.read(&mut buffer).unwrap();
    assert!(read > 0, "server sent nothing back");
    let mut response = buffer[..read].to_vec();
    session.encrypt(&mut response);

    let mut packets = PacketBuffer::new();
    packets.push(&response);
    let reply = packets.take_packet().expect("a framed reply");
    assert_eq!(reply.opcode, 27259);
    let definition = registry.get("S_CHECK_VERSION").unwrap();
    let decoded = tera_protocol::value::read(definition, &reply.encode()).unwrap();
    assert_eq!(decoded.get("ok"), Some(&Value::Uint(1)));

    drop(stream);
    server.join().unwrap();
}
