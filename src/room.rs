use rand::Rng;

/// Generate a random room ID: 8 bytes hex-encoded (16 lowercase hex chars).
pub fn generate_room_id() -> String {
    let bytes: [u8; 8] = rand::thread_rng().gen();
    hex::encode(bytes)
}

fn hex_encode_byte(b: u8) -> [char; 2] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    [HEX[(b >> 4) as usize] as char, HEX[(b & 0xF) as usize] as char]
}

mod hex {
    pub fn encode(bytes: [u8; 8]) -> String {
        let mut s = String::with_capacity(16);
        for b in bytes {
            let [hi, lo] = super::hex_encode_byte(b);
            s.push(hi);
            s.push(lo);
        }
        s
    }
}

/// Build the share link shown to the user.
pub fn build_share_link(server_url: &str, room_id: &str) -> String {
    let server = server_url.trim_end_matches('/');
    format!("entangle join {server}/r/{room_id}")
}

/// Parse the room ID from a join URL like `wss://relay.example.com/r/<room_id>`.
pub fn parse_room_id(url: &str) -> Option<String> {
    let path = url::Url::parse(url).ok()?.path().to_string();
    // Expect path to be /r/<room_id>
    let room = path.strip_prefix("/r/")?;
    if room.is_empty() {
        None
    } else {
        Some(room.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_id_is_16_hex_chars() {
        let id = generate_room_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn room_ids_are_unique() {
        let ids: Vec<_> = (0..100).map(|_| generate_room_id()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len());
    }

    #[test]
    fn share_link_format() {
        let link = build_share_link("wss://relay.example.com", "abc123");
        assert_eq!(link, "entangle join wss://relay.example.com/r/abc123");
    }

    #[test]
    fn parse_room_id_valid() {
        let id = parse_room_id("wss://relay.example.com/r/a3f8c2e9b1d4f067");
        assert_eq!(id, Some("a3f8c2e9b1d4f067".to_string()));
    }

    #[test]
    fn parse_room_id_missing_prefix() {
        assert!(parse_room_id("wss://relay.example.com/a3f8").is_none());
    }

    #[test]
    fn parse_room_id_empty_room() {
        assert!(parse_room_id("wss://relay.example.com/r/").is_none());
    }
}
