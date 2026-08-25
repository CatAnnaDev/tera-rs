use tera_protocol::{Cryptor, OpcodeMap, Session, Sha0, MODERN};

fn digest_hex(data: &[u8]) -> String {
    Sha0::digest(data)
        .iter()
        .map(|word| format!("{word:08x}"))
        .collect()
}

#[test]
fn sha0_matches_reference_vectors() {
    assert_eq!(
        digest_hex(b"abc"),
        "0164b8a914cd2a5e74c4f7ff082c4d97f1edf880"
    );
    assert_eq!(
        digest_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "d2516ee1acfa5baf33dfc1c471e438449ef134c8"
    );
}

#[test]
fn cryptor_is_a_deterministic_keystream() {
    let key = [7u8; 128];
    let mut first = Cryptor::new(&key);
    let mut second = Cryptor::new(&key);
    let mut plain = *b"C_CHECK_VERSION payload that is not aligned";
    let original = plain;
    first.apply(&mut plain);
    assert_ne!(plain, original);
    second.apply(&mut plain);
    assert_eq!(plain, original);
}

#[test]
fn cryptor_keeps_state_across_unaligned_calls() {
    let key = [3u8; 128];
    let mut whole = Cryptor::new(&key);
    let mut split = Cryptor::new(&key);
    let mut reference = [0u8; 37];
    let mut chunks = reference;
    whole.apply(&mut reference);
    let (left, right) = chunks.split_at_mut(5);
    split.apply(left);
    split.apply(right);
    assert_eq!(reference, chunks);
}

#[test]
fn session_keys_are_symmetric() {
    let client_first = [1u8; 128];
    let client_second = [2u8; 128];
    let server_first = [3u8; 128];
    let server_second = [4u8; 128];
    let mut server = Session::new(
        &client_first,
        &client_second,
        &server_first,
        &server_second,
        MODERN,
    );
    let mut client = Session::new(
        &client_first,
        &client_second,
        &server_first,
        &server_second,
        MODERN,
    );
    let mut message = b"S_LOGIN body".to_vec();
    let original = message.clone();
    server.encrypt(&mut message);
    assert_ne!(message, original);
    client.decrypt(&mut message);
    assert_ne!(message, original);
}

#[test]
fn opcode_map_parses() {
    let map = OpcodeMap::parse("# comment\nC_CHECK_VERSION 19900\nS_LOGIN 37454\n").unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map.code("C_CHECK_VERSION"), Some(19900));
    assert_eq!(map.name(37454), Some("S_LOGIN"));
    assert!(map.code("S_MISSING").is_none());
}

#[test]
fn framing_round_trips() {
    use tera_protocol::{Packet, PacketBuffer};
    let first = Packet::new(19900, vec![1, 2, 3, 4, 5]);
    let second = Packet::new(37454, Vec::new());
    let mut wire = first.encode();
    wire.extend_from_slice(&second.encode());
    assert_eq!(u16::from_le_bytes([wire[0], wire[1]]) as usize, first.len());

    let mut buffer = PacketBuffer::new();
    buffer.push(&wire[..3]);
    assert!(buffer.take_packet().is_none());
    buffer.push(&wire[3..]);
    assert_eq!(buffer.take_packet(), Some(first));
    assert_eq!(buffer.take_packet(), Some(second));
    assert!(buffer.take_packet().is_none());
}

#[test]
fn handshake_walks_through_both_keys() {
    use tera_protocol::{random_key, ServerHandshake, Stage, Step, MAGIC};
    let mut handshake = ServerHandshake::new(random_key(), random_key());
    assert_eq!(handshake.greeting(), MAGIC.to_vec());
    assert_eq!(handshake.stage(), Stage::AwaitingClientKeyOne);
    assert!(matches!(handshake.feed(&[9u8; 64]), Step::Wait));
    let step = handshake.feed(&[9u8; 64]);
    match step {
        Step::Send(key) => assert_eq!(key.len(), 128),
        _ => panic!("expected the first server key"),
    }
    assert_eq!(handshake.stage(), Stage::AwaitingClientKeyTwo);
    assert!(matches!(
        handshake.feed(&[4u8; 128]),
        Step::Established(_)
    ));
    assert_eq!(handshake.stage(), Stage::Ready);
}

#[test]
fn random_keys_differ() {
    use tera_protocol::random_key;
    assert_ne!(random_key(), random_key());
}
