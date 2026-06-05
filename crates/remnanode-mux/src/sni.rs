/// Parse SNI hostname from a raw TLS ClientHello buffer.
/// Returns None if the buffer is too short or SNI extension is not found.
pub fn parse_sni(buf: &[u8]) -> Option<String> {
    // TLS record header: ContentType(1) + Version(2) + Length(2)
    if buf.len() < 5 {
        return None;
    }

    // Must be a Handshake record (0x16)
    if buf[0] != 0x16 {
        return None;
    }

    let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if buf.len() < 5 + record_len {
        return None;
    }

    let handshake = &buf[5..];

    // HandshakeType must be ClientHello (0x01)
    if handshake.is_empty() || handshake[0] != 0x01 {
        return None;
    }

    // HandshakeLength is 3 bytes
    if handshake.len() < 4 {
        return None;
    }

    let hello = &handshake[4..];

    // clientVersion(2) + random(32)
    if hello.len() < 34 {
        return None;
    }

    let mut offset = 34;

    // sessionID: 1 byte length + data
    if hello.len() <= offset {
        return None;
    }
    let session_id_len = hello[offset] as usize;
    offset += 1 + session_id_len;

    // cipherSuites: 2 byte length + data
    if hello.len() < offset + 2 {
        return None;
    }
    let cipher_len = u16::from_be_bytes([hello[offset], hello[offset + 1]]) as usize;
    offset += 2 + cipher_len;

    // compressionMethods: 1 byte length + data
    if hello.len() < offset + 1 {
        return None;
    }
    let comp_len = hello[offset] as usize;
    offset += 1 + comp_len;

    // extensions: 2 byte total length
    if hello.len() < offset + 2 {
        return None;
    }
    let _extensions_len = u16::from_be_bytes([hello[offset], hello[offset + 1]]) as usize;
    offset += 2;

    // Walk extensions looking for SNI (type 0x0000)
    while offset + 4 <= hello.len() {
        let ext_type = u16::from_be_bytes([hello[offset], hello[offset + 1]]);
        let ext_len = u16::from_be_bytes([hello[offset + 2], hello[offset + 3]]) as usize;
        offset += 4;

        if offset + ext_len > hello.len() {
            break;
        }

        if ext_type == 0x0000 {
            // SNI extension
            let ext_data = &hello[offset..offset + ext_len];

            // list_length(2) + server_name_type(1) + name_length(2) + name
            if ext_data.len() < 5 {
                break;
            }

            let _list_len = u16::from_be_bytes([ext_data[0], ext_data[1]]);
            let name_type = ext_data[2]; // 0x00 = hostname
            if name_type != 0x00 {
                break;
            }

            let name_len = u16::from_be_bytes([ext_data[3], ext_data[4]]) as usize;
            if ext_data.len() < 5 + name_len {
                break;
            }

            return String::from_utf8(ext_data[5..5 + name_len].to_vec()).ok();
        }

        offset += ext_len;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sni_no_data() {
        assert_eq!(parse_sni(&[]), None);
    }

    #[test]
    fn test_parse_sni_not_handshake() {
        assert_eq!(parse_sni(&[0x17, 0x03, 0x01, 0x00, 0x01, 0x00]), None);
    }
}
