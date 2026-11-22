use tera_protocol::value::SkillId;

pub const GREET_SKILL_ID: u32 = 60401301;

pub fn skill_raw() -> u64 {
    SkillId::player(GREET_SKILL_ID).raw()
}

pub fn is_greet(skill_raw: u64) -> bool {
    SkillId::from_raw(skill_raw).id == GREET_SKILL_ID
}

pub fn face(from: [f32; 3], to: [f32; 3]) -> i64 {
    let angle = (to[1] - from[1]).atan2(to[0] - from[0]);
    (angle * 32768.0 / std::f32::consts::PI).round() as i64
}

fn forward(loc: [f32; 3], angle_raw: i64) -> [f32; 3] {
    let radians = (angle_raw as i16 as f32) * std::f32::consts::PI / 32768.0;
    let reach = 250.0;
    [loc[0] + reach * radians.cos(), loc[1] + reach * radians.sin(), loc[2]]
}

const OPCODE: u16 = 49469;
const GARBAGE: [u8; 8] = [0x10, 0x00, 0x00, 0x00, 0x35, 0x74, 0xbf, 0xff];

fn frame(body: Vec<u8>) -> Vec<u8> {
    let mut packet = Vec::with_capacity(4 + body.len());
    packet.extend_from_slice(&((4 + body.len()) as u16).to_le_bytes());
    packet.extend_from_slice(&OPCODE.to_le_bytes());
    packet.extend_from_slice(&body);
    packet
}

fn head(body: &mut Vec<u8>, angle_raw: i64, loc: [f32; 3], hold: bool, targets: u16, targets_off: u16, endpoints_off: u16) {
    body.extend_from_slice(&GARBAGE);
    body.extend_from_slice(&targets.to_le_bytes());
    body.extend_from_slice(&targets_off.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes());
    body.extend_from_slice(&endpoints_off.to_le_bytes());
    body.extend_from_slice(&skill_raw().to_le_bytes());
    for value in loc {
        body.extend_from_slice(&value.to_le_bytes());
    }
    body.extend_from_slice(&(angle_raw as i16).to_le_bytes());
    body.push(u8::from(hold));
}

pub fn build_with(loc: [f32; 3], angle_raw: i64, hold: bool) -> Vec<u8> {
    let endpoint = forward(loc, angle_raw);
    let mut body = Vec::with_capacity(55);
    head(&mut body, angle_raw, loc, hold, 0, 0, 0x2b);
    body.extend_from_slice(&0x2bu32.to_le_bytes());
    for value in endpoint {
        body.extend_from_slice(&value.to_le_bytes());
    }
    frame(body)
}

pub fn build_targeted(loc: [f32; 3], angle_raw: i64, target_gid: u64, endpoint: [f32; 3]) -> Vec<u8> {
    let mut body = Vec::with_capacity(75);
    head(&mut body, angle_raw, loc, false, 1, 0x2b, 0x3f);
    body.extend_from_slice(&0x2bu32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&target_gid.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&0x3fu32.to_le_bytes());
    for value in endpoint {
        body.extend_from_slice(&value.to_le_bytes());
    }
    frame(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_is_player_60401301() {
        assert_eq!(skill_raw(), u64::from(GREET_SKILL_ID) | 0x1000_0000);
        assert!(is_greet(skill_raw()));
    }

    #[test]
    fn raw_matches_capture_size_and_skill() {
        let packet = build_with([37661.55, -87634.86, -452.3], 25408, false);
        assert_eq!(packet.len(), 59);
        assert_eq!(u16::from_le_bytes([packet[0], packet[1]]), 59);
        assert_eq!(u16::from_le_bytes([packet[2], packet[3]]), OPCODE);
        assert_eq!(&packet[20..28], &skill_raw().to_le_bytes());
    }

    #[test]
    fn targeted_matches_capture_layout() {
        let gid = 0x0080_0000_01f4_0972u64;
        let packet = build_targeted([37661.55, -87634.86, -452.3], 25408, gid, [37700.0, -87600.0, -451.0]);
        assert_eq!(packet.len(), 79);
        assert_eq!(u16::from_le_bytes([packet[0], packet[1]]), 79);
        assert_eq!(&packet[12..16], &[0x01, 0x00, 0x2b, 0x00]);
        assert_eq!(&packet[16..20], &[0x01, 0x00, 0x3f, 0x00]);
        assert_eq!(&packet[51..59], &gid.to_le_bytes());
    }
}
