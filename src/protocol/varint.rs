use byteorder::ReadBytesExt;
use std::io::Cursor;

const MAX_VARINT_BYTES: usize = 5;

/// 读取一个 VarInt
/// 使用 Cursor 来方便地从字节切片中读取数据
/// 返回 Option<(i32, usize)>，其中 i32 是解析出的值，usize 是读取的字节数
pub fn read_varint(reader: &mut Cursor<&[u8]>) -> Option<(i32, usize)> {
    let mut num_read = 0;
    let mut result = 0i32;
    let mut read: u8;

    loop {
        if reader.position() >= reader.get_ref().len() as u64 {
            return None; // 数据不足，无法读取一个完整的字节
        }
        read = reader.read_u8().ok()?;
        num_read += 1;

        let value = (read & 0b0111_1111) as i32;
        result |= value << (7 * (num_read - 1));

        if num_read > MAX_VARINT_BYTES {
            return None; // VarInt 太长，格式错误
        }

        if (read & 0b1000_0000) == 0 {
            break;
        }
    }
    Some((result, num_read))
}
