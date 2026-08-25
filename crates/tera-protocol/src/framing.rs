pub const HEADER_LEN: usize = 4;
pub const MAX_PACKET_LEN: usize = 0xffff;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub opcode: u16,
    pub body: Vec<u8>,
}

impl Packet {
    pub fn new(opcode: u16, body: Vec<u8>) -> Self {
        Self { opcode, body }
    }

    pub fn len(&self) -> usize {
        HEADER_LEN + self.body.len()
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        out.extend_from_slice(&(self.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.opcode.to_le_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

#[derive(Default)]
pub struct PacketBuffer {
    pending: Vec<u8>,
}

impl PacketBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) {
        self.pending.extend_from_slice(data);
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    pub fn take_packet(&mut self) -> Option<Packet> {
        if self.pending.len() < HEADER_LEN {
            return None;
        }
        let size = u16::from_le_bytes([self.pending[0], self.pending[1]]) as usize;
        if size < HEADER_LEN {
            self.pending.clear();
            return None;
        }
        if self.pending.len() < size {
            return None;
        }
        let opcode = u16::from_le_bytes([self.pending[2], self.pending[3]]);
        let body = self.pending[HEADER_LEN..size].to_vec();
        self.pending.drain(..size);
        Some(Packet { opcode, body })
    }
}
