/// y-websocket sync protocol message encoding/decoding.
///
/// Wire format uses lib0 variable-length integer encoding. Each WebSocket binary
/// frame starts with a message-type varint (0 = sync), followed by a sync
/// subtype varint (0=SyncStep1, 1=SyncStep2, 2=Update), followed by the payload
/// as a length-prefixed byte array.

const MSG_SYNC: u64 = 0;
const SYNC_STEP1: u64 = 0;
const SYNC_STEP2: u64 = 1;
const SYNC_UPDATE: u64 = 2;

#[derive(Debug)]
pub enum SyncMessage {
    SyncStep1(Vec<u8>),
    SyncStep2(Vec<u8>),
    Update(Vec<u8>),
}

pub fn encode_sync_step1(state_vector: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    write_varint(&mut buf, MSG_SYNC);
    write_varint(&mut buf, SYNC_STEP1);
    write_var_bytes(&mut buf, state_vector);
    buf
}

pub fn encode_sync_step2(update: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    write_varint(&mut buf, MSG_SYNC);
    write_varint(&mut buf, SYNC_STEP2);
    write_var_bytes(&mut buf, update);
    buf
}

pub fn encode_update(update: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    write_varint(&mut buf, MSG_SYNC);
    write_varint(&mut buf, SYNC_UPDATE);
    write_var_bytes(&mut buf, update);
    buf
}

pub fn decode_message(data: &[u8]) -> Option<SyncMessage> {
    let mut pos = 0usize;
    let msg_type = read_varint(data, &mut pos)?;
    if msg_type != MSG_SYNC {
        return None;
    }
    let sync_type = read_varint(data, &mut pos)?;
    let payload = read_var_bytes(data, &mut pos)?.to_vec();
    match sync_type {
        SYNC_STEP1 => Some(SyncMessage::SyncStep1(payload)),
        SYNC_STEP2 => Some(SyncMessage::SyncStep2(payload)),
        SYNC_UPDATE => Some(SyncMessage::Update(payload)),
        _ => None,
    }
}

fn write_varint(buf: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

fn write_var_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    write_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

fn read_varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        if *pos >= data.len() {
            return None;
        }
        let byte = data[*pos] as u64;
        *pos += 1;
        result |= (byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    Some(result)
}

fn read_var_bytes<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    let len = read_varint(data, pos)? as usize;
    if *pos + len > data.len() {
        return None;
    }
    let result = &data[*pos..*pos + len];
    *pos += len;
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_sync_step1() {
        let sv = vec![1, 2, 3, 4];
        let encoded = encode_sync_step1(&sv);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, SyncMessage::SyncStep1(v) if v == sv));
    }

    #[test]
    fn round_trip_sync_step2() {
        let update = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let encoded = encode_sync_step2(&update);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, SyncMessage::SyncStep2(v) if v == update));
    }

    #[test]
    fn round_trip_update() {
        let update = vec![42u8; 128];
        let encoded = encode_update(&update);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, SyncMessage::Update(v) if v == update));
    }

    #[test]
    fn unknown_msg_type_returns_none() {
        // message type 1 = awareness, not sync
        let mut buf = Vec::new();
        write_varint(&mut buf, 1);
        write_varint(&mut buf, 0);
        write_var_bytes(&mut buf, &[1, 2, 3]);
        assert!(decode_message(&buf).is_none());
    }

    #[test]
    fn truncated_message_returns_none() {
        assert!(decode_message(&[]).is_none());
        assert!(decode_message(&[0]).is_none());
    }

    #[test]
    fn varint_multibyte() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 300);
        let mut pos = 0;
        let v = read_varint(&buf, &mut pos).unwrap();
        assert_eq!(v, 300);
        assert_eq!(pos, buf.len());
    }
}
