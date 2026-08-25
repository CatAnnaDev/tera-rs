pub struct Sha0 {
    digest: [u32; 5],
    block: [u8; 64],
    index: usize,
    length_low: u32,
    length_high: u32,
}

impl Default for Sha0 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha0 {
    pub fn new() -> Self {
        Self {
            digest: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0],
            block: [0; 64],
            index: 0,
            length_low: 0,
            length_high: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        for byte in data {
            self.block[self.index] = *byte;
            self.index += 1;
            self.length_low = self.length_low.wrapping_add(8);
            if self.length_low == 0 {
                self.length_high = self.length_high.wrapping_add(1);
            }
            if self.index == 64 {
                self.process();
            }
        }
    }

    fn process(&mut self) {
        let mut words = [0u32; 80];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let base = index * 4;
            *word = u32::from_be_bytes([
                self.block[base],
                self.block[base + 1],
                self.block[base + 2],
                self.block[base + 3],
            ]);
        }
        for index in 16..80 {
            words[index] =
                words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16];
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.digest;
        for (round, word) in words.iter().enumerate() {
            let (mixed, constant) = match round {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(e)
                .wrapping_add(*word)
                .wrapping_add(mixed)
                .wrapping_add(constant);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        self.digest[0] = self.digest[0].wrapping_add(a);
        self.digest[1] = self.digest[1].wrapping_add(b);
        self.digest[2] = self.digest[2].wrapping_add(c);
        self.digest[3] = self.digest[3].wrapping_add(d);
        self.digest[4] = self.digest[4].wrapping_add(e);
        self.index = 0;
        self.block = [0; 64];
    }

    pub fn finish(mut self) -> [u32; 5] {
        let length_low = self.length_low;
        let length_high = self.length_high;
        self.block[self.index] = 0x80;
        self.index += 1;
        if self.index > 56 {
            while self.index < 64 {
                self.block[self.index] = 0;
                self.index += 1;
            }
            self.process();
        }
        while self.index < 56 {
            self.block[self.index] = 0;
            self.index += 1;
        }
        self.block[56..60].copy_from_slice(&length_high.to_be_bytes());
        self.block[60..64].copy_from_slice(&length_low.to_be_bytes());
        self.process();
        self.digest
    }

    pub fn digest(data: &[u8]) -> [u32; 5] {
        let mut hasher = Self::new();
        hasher.update(data);
        hasher.finish()
    }
}
