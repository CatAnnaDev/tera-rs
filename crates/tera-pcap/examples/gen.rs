use std::io::Write;
use tera_protocol::{Packet, Session, MODERN, MAGIC};

fn be16(value: u16) -> [u8; 2] {
    value.to_be_bytes()
}

fn frame(src_port: u16, dst_port: u16, seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut ethernet = vec![0u8; 12];
    ethernet.extend_from_slice(&[0x08, 0x00]);

    let mut tcp = Vec::new();
    tcp.extend_from_slice(&be16(src_port));
    tcp.extend_from_slice(&be16(dst_port));
    tcp.extend_from_slice(&seq.to_be_bytes());
    tcp.extend_from_slice(&[0, 0, 0, 0]);
    tcp.push(0x50);
    tcp.push(0x18);
    tcp.extend_from_slice(&[0xff, 0xff, 0, 0, 0, 0]);
    tcp.extend_from_slice(payload);

    let total = 20 + tcp.len();
    let mut ip = vec![0x45, 0x00];
    ip.extend_from_slice(&be16(total as u16));
    ip.extend_from_slice(&[0, 0, 0x40, 0x00, 0x40, 0x06, 0, 0]);
    ip.extend_from_slice(&[192, 168, 1, 185]);
    ip.extend_from_slice(&[31, 204, 136, 149]);
    ip.extend_from_slice(&tcp);

    let mut out = ethernet;
    out.extend_from_slice(&ip);
    out
}

fn record(out: &mut Vec<u8>, frame: &[u8]) {
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    out.extend_from_slice(frame);
}

fn main() {
    let client_first = [0x11u8; 128];
    let client_second = [0x22u8; 128];
    let server_first = [0x33u8; 128];
    let server_second = [0x44u8; 128];

    let check_body = vec![
        0x02, 0x00, 0x08, 0x00, 0x08, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0xcc, 0xbc, 0x05,
        0x00, 0x14, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xc1, 0xbc, 0x05, 0x00,
    ];
    let client_plain = Packet::new(19900, check_body).encode();
    let server_plain = Packet::new(19901, vec![0x01]).encode();

    let mut cs = Session::new(&client_first, &client_second, &server_first, &server_second, MODERN);
    let mut client_cipher = client_plain.clone();
    cs.decrypt(&mut client_cipher);

    let mut sc = Session::new(&client_first, &client_second, &server_first, &server_second, MODERN).swapped();
    let mut server_cipher = server_plain.clone();
    sc.decrypt(&mut server_cipher);

    let mut client_stream = Vec::new();
    client_stream.extend_from_slice(&client_first);
    client_stream.extend_from_slice(&client_second);
    client_stream.extend_from_slice(&client_cipher);

    let mut server_stream = Vec::new();
    server_stream.extend_from_slice(&MAGIC);
    server_stream.extend_from_slice(&server_first);
    server_stream.extend_from_slice(&server_second);
    server_stream.extend_from_slice(&server_cipher);

    let mut pcap = Vec::new();
    pcap.extend_from_slice(&0xa1b2c3d4u32.to_le_bytes());
    pcap.extend_from_slice(&2u16.to_le_bytes());
    pcap.extend_from_slice(&4u16.to_le_bytes());
    pcap.extend_from_slice(&[0; 8]);
    pcap.extend_from_slice(&65535u32.to_le_bytes());
    pcap.extend_from_slice(&1u32.to_le_bytes());

    record(&mut pcap, &frame(50000, 7800, 1000, &client_stream));
    record(&mut pcap, &frame(7800, 50000, 2000, &server_stream));

    let path = std::env::args().nth(1).unwrap_or_else(|| "test.pcap".to_string());
    std::fs::File::create(&path).unwrap().write_all(&pcap).unwrap();
    eprintln!("écrit {path}");
}
