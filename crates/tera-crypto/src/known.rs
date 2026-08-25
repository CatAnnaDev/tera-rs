use crate::KeyIv;

pub struct KnownKey {
    pub label: &'static str,
    pub key_hex: &'static str,
    pub iv_hex: &'static str,
}

impl KnownKey {
    pub fn keyiv(&self) -> KeyIv {
        KeyIv::from_hex(self.key_hex, self.iv_hex).expect("built-in key table must be valid hex")
    }
}

static TABLE: &[KnownKey] = &[
    KnownKey {
        label: "v100.02 GF (TERA Europe Classic, verified)",
        key_hex: "1C01C904FF76FF06C211187E197B5716",
        iv_hex: "396C342C52A0C12D511DD0209F90CA7D",
    },
    KnownKey {
        label: "EU build (community published, unverified here)",
        key_hex: "4B9062798671360CB0C7C56686E1AE2A",
        iv_hex: "7E2B1F0258B86F2FB87C6A51A8B28D70",
    },
];

pub fn known_keys() -> &'static [KnownKey] {
    TABLE
}
