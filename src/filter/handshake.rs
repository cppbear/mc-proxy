use crate::protocol::varint;
use byteorder::{BigEndian, ReadBytesExt};
use std::io::Cursor;

/// 主验证函数，检查数据是否为有效的 Minecraft 握手包
pub fn is_valid_handshake(data: &[u8]) -> bool {
    // 使用 Cursor 包装数据，方便按字节读取
    let mut cursor = Cursor::new(data);

    // 整个TCP载荷是否以一个有效的VarInt（Packet Length）开头
    let (packet_length, length_bytes_read) = match varint::read_varint(&mut cursor) {
        Some(result) => result,
        None => {
            // 连包长度都无法解析，判定为无效
            tracing::error!("Failed to parse packet length");
            return false;
        }
    };

    // 检查包长度是否与实际数据长度匹配
    // packet_length 是指包ID+后续数据的长度
    if packet_length as usize != data.len() - length_bytes_read {
        tracing::error!(
            "Packet length mismatch. Expected: {}, Got: {}",
            data.len() - length_bytes_read,
            packet_length
        );
        return false;
    }

    // 其Packet ID（紧跟在长度之后的VarInt）是否等于0x00
    let (packet_id, _) = match varint::read_varint(&mut cursor) {
        Some(result) => result,
        None => {
            tracing::error!("Failed to parse packet ID");
            return false; // 无法解析 Packet ID
        }
    };

    if packet_id != 0x00 {
        // 不是握手包（ID不为0x00）
        tracing::error!("Packet ID mismatch. Expected: 0x00, Got: {:#x}", packet_id);
        return false;
    }

    // 在包ID为0x00的情况下，验证后续字段
    // 字段顺序: Protocol Version (VarInt), Server Address (String), Server Port (u16), Next State (VarInt)

    // 字段 a: Protocol Version (VarInt)
    let (protocol_version, _) = match varint::read_varint(&mut cursor) {
        Some(result) => result,
        None => {
            tracing::error!("Failed to parse protocol version");
            return false;
        }
    };

    // 字段 b: Server Address (String)
    // Minecraft 字符串以一个 VarInt 长度作为前缀
    let (string_len, _) = match varint::read_varint(&mut cursor) {
        Some(result) => result,
        None => {
            tracing::error!("Failed to parse server address length");
            return false;
        }
    };
    let string_len = string_len as usize;
    let current_pos = cursor.position() as usize;
    // 检查是否有足够的数据来读取字符串内容
    if current_pos + string_len > data.len() {
        tracing::error!("Insufficient data for server address");
        return false;
    }
    // 读取字符串内容
    let server_address = match String::from_utf8(data[current_pos..current_pos + string_len].to_vec()) {
        Ok(s) => s,
        Err(_) => {
            tracing::error!("Invalid UTF-8 in server address");
            return false; // Invalid UTF-8
        }
    };
    // 更新游标位置
    cursor.set_position((current_pos + string_len) as u64);

    // 字段 c: Server Port (u16)
    // 这是一个固定的2字节大端序整数
    let server_port = match cursor.read_u16::<BigEndian>() {
        Ok(port) => port,
        Err(_) => {
            tracing::error!("Failed to read server port");
            return false;
        }
    };

    // 字段 d: Next State (VarInt)
    // 下一个状态只能是 1 (Status) 或 2 (Login)
    let (next_state, _) = match varint::read_varint(&mut cursor) {
        Some(result) => result,
        None => {
            tracing::error!("Failed to parse next state");
            return false;
        }
    };
    if !(next_state == 1 || next_state == 2) {
        tracing::error!("Invalid next state. Expected 1 or 2, Got: {}", next_state);
        return false;
    }

    // 最后，检查是否所有数据都已“消费”完毕
    // 如果游标位置不等于数据总长度，说明包尾有额外的数据，格式错误
    if cursor.position() != data.len() as u64 {
        tracing::error!(
            "Extra data at the end of the packet. Cursor position: {}, Data length: {}",
            cursor.position(),
            data.len()
        );
        return false;
    }

    // 所有检查都通过了，记录日志
    tracing::info!(
        packet_length,
        packet_id,
        protocol_version,
        server_address = server_address.as_str(),
        server_port,
        next_state,
        "Valid handshake packet received"
    );

    true
}
