use crate::sha0::Sha0;

const KEY_SIZES: [(usize, usize); 3] = [(55, 31), (57, 50), (58, 39)];
const KEY_BYTES: [usize; 3] = [220, 228, 232];

struct Register {
    size: usize,
    position_one: usize,
    position_two: usize,
    key: u32,
    sum: u32,
    buffer: Vec<u32>,
}

impl Register {
    fn new(size: usize, max_position: usize) -> Self {
        Self {
            size,
            position_one: 0,
            position_two: max_position,
            key: 0,
            sum: 0,
            buffer: vec![0; size],
        }
    }

    fn step(&mut self) {
        let first = self.buffer[self.position_one];
        let second = self.buffer[self.position_two];
        let smaller = first.min(second);
        self.sum = first.wrapping_add(second);
        self.key = u32::from(smaller > self.sum);
        self.position_one = (self.position_one + 1) % self.size;
        self.position_two = (self.position_two + 1) % self.size;
    }
}

pub struct Cryptor {
    registers: [Register; 3],
    change_data: u32,
    change_length: usize,
}

impl Cryptor {
    pub fn new(key: &[u8; 128]) -> Self {
        let mut expanded = [0u8; 680];
        for (index, slot) in expanded.iter_mut().enumerate() {
            *slot = key[index % 128];
        }
        expanded[0] = 128;
        for offset in (0..680).step_by(20) {
            let digest = Sha0::digest(&expanded);
            for (word, value) in digest.iter().enumerate() {
                expanded[offset + word * 4..offset + word * 4 + 4]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
        let mut registers = [
            Register::new(KEY_SIZES[0].0, KEY_SIZES[0].1),
            Register::new(KEY_SIZES[1].0, KEY_SIZES[1].1),
            Register::new(KEY_SIZES[2].0, KEY_SIZES[2].1),
        ];
        let mut base = 0usize;
        for (index, register) in registers.iter_mut().enumerate() {
            let bytes = KEY_BYTES[index];
            for word in 0..bytes / 4 {
                let start = base + word * 4;
                register.buffer[word] = u32::from_le_bytes([
                    expanded[start],
                    expanded[start + 1],
                    expanded[start + 2],
                    expanded[start + 3],
                ]);
            }
            base += bytes;
        }
        Self {
            registers,
            change_data: 0,
            change_length: 0,
        }
    }

    fn vote(&self) -> u32 {
        let first = self.registers[0].key;
        let second = self.registers[1].key;
        let third = self.registers[2].key;
        (first & second) | (third & (first | second))
    }

    pub fn apply(&mut self, data: &mut [u8]) {
        let size = data.len();
        let leading = size.min(self.change_length);
        for (index, byte) in data.iter_mut().take(leading).enumerate() {
            let shift = 8 * (4 - self.change_length + index);
            *byte ^= (self.change_data >> shift) as u8;
        }
        self.change_length -= leading;
        let remaining = size - leading;

        let mut position = leading;
        while position + 4 <= size {
            let vote = self.vote();
            for register in self.registers.iter_mut() {
                if vote == register.key {
                    register.step();
                }
                let sum = register.sum;
                data[position] ^= sum as u8;
                data[position + 1] ^= (sum >> 8) as u8;
                data[position + 2] ^= (sum >> 16) as u8;
                data[position + 3] ^= (sum >> 24) as u8;
            }
            position += 4;
        }

        let tail = remaining & 3;
        if tail != 0 {
            let vote = self.vote();
            self.change_data = 0;
            for register in self.registers.iter_mut() {
                if vote == register.key {
                    register.step();
                }
                self.change_data ^= register.sum;
            }
            for index in 0..tail {
                data[size - tail + index] ^= (self.change_data >> (index * 8)) as u8;
            }
            self.change_length = 4 - tail;
        }
    }
}
