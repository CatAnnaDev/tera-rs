use crate::cryptor::Cryptor;

pub const KEY_LEN: usize = 128;

#[derive(Clone, Copy)]
pub struct Constants {
    pub server_first_shift: i32,
    pub client_second_shift: i32,
    pub server_second_shift: i32,
}

pub const MODERN: Constants = Constants {
    server_first_shift: -67,
    client_second_shift: 29,
    server_second_shift: -41,
};

pub const LEGACY: Constants = Constants {
    server_first_shift: -31,
    client_second_shift: 17,
    server_second_shift: -79,
};

fn shift(source: &[u8; KEY_LEN], amount: i32) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    if amount > 0 {
        let split = amount as usize % KEY_LEN;
        out[..KEY_LEN - split].copy_from_slice(&source[split..]);
        out[KEY_LEN - split..].copy_from_slice(&source[..split]);
    } else {
        let split = (-amount) as usize % KEY_LEN;
        out[..split].copy_from_slice(&source[KEY_LEN - split..]);
        out[split..].copy_from_slice(&source[..KEY_LEN - split]);
    }
    out
}

fn xor(left: &[u8; KEY_LEN], right: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    for index in 0..KEY_LEN {
        out[index] = left[index] ^ right[index];
    }
    out
}

pub struct Session {
    encryptor: Cryptor,
    decryptor: Cryptor,
}

impl Session {
    pub fn new(
        client_first: &[u8; KEY_LEN],
        client_second: &[u8; KEY_LEN],
        server_first: &[u8; KEY_LEN],
        server_second: &[u8; KEY_LEN],
        constants: Constants,
    ) -> Self {
        let shifted = shift(server_first, constants.server_first_shift);
        let mixed = xor(&shifted, client_first);
        let shifted = shift(client_second, constants.client_second_shift);
        let decrypt_key = xor(&shifted, &mixed);
        let mut decryptor = Cryptor::new(&decrypt_key);

        let mut encrypt_key = shift(server_second, constants.server_second_shift);
        decryptor.apply(&mut encrypt_key);
        let encryptor = Cryptor::new(&encrypt_key);
        Self {
            encryptor,
            decryptor,
        }
    }

    pub fn decrypt(&mut self, data: &mut [u8]) {
        self.decryptor.apply(data);
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        self.encryptor.apply(data);
    }

    pub fn swapped(self) -> Self {
        let Self {
            encryptor,
            decryptor,
        } = self;
        Self {
            encryptor: decryptor,
            decryptor: encryptor,
        }
    }

    pub fn split(self) -> (Encrypting, Decrypting) {
        let Self {
            encryptor,
            decryptor,
        } = self;
        (Encrypting { encryptor }, Decrypting { decryptor })
    }
}

pub struct Encrypting {
    encryptor: Cryptor,
}

impl Encrypting {
    pub fn apply(&mut self, data: &mut [u8]) {
        self.encryptor.apply(data);
    }
}

pub struct Decrypting {
    decryptor: Cryptor,
}

impl Decrypting {
    pub fn apply(&mut self, data: &mut [u8]) {
        self.decryptor.apply(data);
    }
}
