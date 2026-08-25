use crate::session::{Constants, Session, KEY_LEN, MODERN};

pub const MAGIC: [u8; 4] = [1, 0, 0, 0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    AwaitingClientKeyOne,
    AwaitingClientKeyTwo,
    Ready,
}

pub enum Step {
    Send(Vec<u8>),
    Established(Box<Session>),
    Wait,
}

pub struct ServerHandshake {
    stage: Stage,
    constants: Constants,
    client_first: [u8; KEY_LEN],
    client_second: [u8; KEY_LEN],
    server_first: [u8; KEY_LEN],
    server_second: [u8; KEY_LEN],
    pending: Vec<u8>,
}

impl ServerHandshake {
    pub fn new(server_first: [u8; KEY_LEN], server_second: [u8; KEY_LEN]) -> Self {
        Self {
            stage: Stage::AwaitingClientKeyOne,
            constants: MODERN,
            client_first: [0; KEY_LEN],
            client_second: [0; KEY_LEN],
            server_first,
            server_second,
            pending: Vec::new(),
        }
    }

    pub fn with_constants(mut self, constants: Constants) -> Self {
        self.constants = constants;
        self
    }

    pub fn greeting(&self) -> Vec<u8> {
        MAGIC.to_vec()
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn feed(&mut self, data: &[u8]) -> Step {
        self.pending.extend_from_slice(data);
        if self.pending.len() < KEY_LEN {
            return Step::Wait;
        }
        let key: [u8; KEY_LEN] = self.pending[..KEY_LEN].try_into().expect("checked length");
        self.pending.drain(..KEY_LEN);
        match self.stage {
            Stage::AwaitingClientKeyOne => {
                self.client_first = key;
                self.stage = Stage::AwaitingClientKeyTwo;
                Step::Send(self.server_first.to_vec())
            }
            Stage::AwaitingClientKeyTwo => {
                self.client_second = key;
                self.stage = Stage::Ready;
                Step::Established(Box::new(Session::new(
                    &self.client_first,
                    &self.client_second,
                    &self.server_first,
                    &self.server_second,
                    self.constants,
                )))
            }
            Stage::Ready => Step::Wait,
        }
    }

    pub fn server_second(&self) -> &[u8; KEY_LEN] {
        &self.server_second
    }

    pub fn leftover(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

pub struct ClientHandshake {
    constants: Constants,
    client_first: [u8; KEY_LEN],
    client_second: [u8; KEY_LEN],
}

impl ClientHandshake {
    pub fn new(client_first: [u8; KEY_LEN], client_second: [u8; KEY_LEN]) -> Self {
        Self {
            constants: MODERN,
            client_first,
            client_second,
        }
    }

    pub fn with_constants(mut self, constants: Constants) -> Self {
        self.constants = constants;
        self
    }

    pub fn first(&self) -> &[u8; KEY_LEN] {
        &self.client_first
    }

    pub fn second(&self) -> &[u8; KEY_LEN] {
        &self.client_second
    }

    pub fn finish(
        &self,
        server_first: &[u8; KEY_LEN],
        server_second: &[u8; KEY_LEN],
    ) -> Session {
        Session::new(
            &self.client_first,
            &self.client_second,
            server_first,
            server_second,
            self.constants,
        )
        .swapped()
    }
}

pub fn random_key() -> [u8; KEY_LEN] {
    use std::io::Read;
    let mut key = [0u8; KEY_LEN];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        if file.read_exact(&mut key).is_ok() {
            return key;
        }
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0x1234_5678);
    let mut state = seed | 1;
    for slot in key.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *slot = (state >> 24) as u8;
    }
    key
}

#[cfg(test)]
mod proxy_tests {
    use super::*;

    #[test]
    fn a_client_and_a_server_agree_on_a_stream() {
        let (server_first, server_second) = ([7u8; KEY_LEN], [9u8; KEY_LEN]);
        let (client_first, client_second) = ([3u8; KEY_LEN], [5u8; KEY_LEN]);

        let mut server = ServerHandshake::new(server_first, server_second);
        assert!(matches!(server.feed(&client_first), Step::Send(_)));
        let Step::Established(mut server_session) = server.feed(&client_second) else {
            panic!("the second client key must establish the session");
        };

        let client = ClientHandshake::new(client_first, client_second);
        let mut client_session = client.finish(&server_first, &server_second);

        let mut payload = *b"a packet the server sends to the client";
        server_session.encrypt(&mut payload);
        assert_ne!(&payload[..], b"a packet the server sends to the client");
        client_session.decrypt(&mut payload);
        assert_eq!(&payload[..], b"a packet the server sends to the client");
    }

    #[test]
    fn splitting_a_session_matches_the_whole_one() {
        let keys = ([1u8; KEY_LEN], [2u8; KEY_LEN], [3u8; KEY_LEN], [4u8; KEY_LEN]);
        let mut whole = Session::new(&keys.0, &keys.1, &keys.2, &keys.3, MODERN);
        let (mut encrypting, _) = Session::new(&keys.0, &keys.1, &keys.2, &keys.3, MODERN).split();

        let mut one = *b"the same bytes through both paths";
        let mut two = one;
        whole.encrypt(&mut one);
        encrypting.apply(&mut two);
        assert_eq!(one, two);
    }

    #[test]
    fn a_proxy_can_relay_in_both_directions() {
        let (server_first, server_second) = ([7u8; KEY_LEN], [9u8; KEY_LEN]);
        let (client_first, client_second) = ([3u8; KEY_LEN], [5u8; KEY_LEN]);
        let mut server = ServerHandshake::new(server_first, server_second);
        server.feed(&client_first);
        let Step::Established(mut server_session) = server.feed(&client_second) else {
            panic!("session");
        };
        let mut client_session =
            ClientHandshake::new(client_first, client_second).finish(&server_first, &server_second);

        let mut upward = *b"a packet the client sends to the server";
        client_session.encrypt(&mut upward);
        server_session.decrypt(&mut upward);
        assert_eq!(&upward[..], b"a packet the client sends to the server");
    }
}
