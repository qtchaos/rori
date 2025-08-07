use byteorder::{BigEndian, ReadBytesExt};

use std::io::{Cursor, Read};

use crate::ProcessError;

#[derive(Debug)]
pub enum NbtError {
    IoError(std::io::Error),
    InvalidFormat(String),
}

impl From<std::io::Error> for NbtError {
    fn from(error: std::io::Error) -> Self {
        NbtError::IoError(error)
    }
}

impl std::fmt::Display for NbtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NbtError::IoError(e) => write!(f, "IO error: {}", e),
            NbtError::InvalidFormat(msg) => write!(f, "Invalid NBT format: {}", msg),
        }
    }
}

impl std::error::Error for NbtError {}

const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_LONG: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;

/// This parser only searches for the specific field and skips everything else
pub fn extract_inhabited_time(chunk_data: &[u8]) -> Result<Option<i64>, NbtError> {
    // Prefetch the beginning of the chunk data into cache
    if chunk_data.len() >= 64 {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            if is_x86_feature_detected!("sse") {
                std::arch::x86_64::_mm_prefetch(
                    chunk_data.as_ptr() as *const i8,
                    std::arch::x86_64::_MM_HINT_T0,
                );
            }
        }
    }

    let mut cursor = Cursor::new(chunk_data);

    // Read root compound tag
    let tag_type = cursor.read_u8()?;
    if tag_type != TAG_COMPOUND {
        return Err(NbtError::InvalidFormat(
            "Root tag is not a compound".to_string(),
        ));
    }

    // Skip root tag name
    skip_string(&mut cursor)?;

    // Search through the root compound for InhabitedTime
    search_compound(&mut cursor, 2)
}

fn search_compound<R: Read>(reader: &mut R, max_depth: u8) -> Result<Option<i64>, NbtError> {
    search_compound_recursive(reader, max_depth, 0)
}

/// Check if a string matches "InhabitedTime" without allocating
#[inline(always)]
fn is_inhabited_time_string<R: Read>(reader: &mut R) -> Result<bool, NbtError> {
    let length = reader.read_u16::<BigEndian>()? as usize;

    const INHABITED_TIME: &[u8] = b"InhabitedTime";

    if length != INHABITED_TIME.len() {
        skip_bytes(reader, length)?;
        return Ok(false);
    }

    // Read the string data into a buffer
    let mut buffer = [0u8; 13];
    reader.read_exact(&mut buffer)?;

    // Use SIMD-optimized comparison
    Ok(compare_inhabited_time(&buffer))
}

#[inline(always)]
pub fn compare_inhabited_time(buffer: &[u8; 13]) -> bool {
    const INHABITED_TIME: &[u8] = b"InhabitedTime";

    // Compare in 8-byte chunks using u64
    // "InhabitedTime" = 13 bytes, so we compare 8 bytes + 5 bytes

    // First 8 bytes: "Inhabite"
    let chunk1_buffer = u64::from_ne_bytes([
        buffer[0], buffer[1], buffer[2], buffer[3], buffer[4], buffer[5], buffer[6], buffer[7],
    ]);
    let chunk1_target = u64::from_ne_bytes([
        INHABITED_TIME[0],
        INHABITED_TIME[1],
        INHABITED_TIME[2],
        INHABITED_TIME[3],
        INHABITED_TIME[4],
        INHABITED_TIME[5],
        INHABITED_TIME[6],
        INHABITED_TIME[7],
    ]);

    if chunk1_buffer != chunk1_target {
        return false;
    }

    // Remaining 5 bytes: "dTime"
    let chunk2_buffer = u64::from_ne_bytes([
        buffer[8], buffer[9], buffer[10], buffer[11], buffer[12], 0, 0, 0,
    ]);
    let chunk2_target = u64::from_ne_bytes([
        INHABITED_TIME[8],
        INHABITED_TIME[9],
        INHABITED_TIME[10],
        INHABITED_TIME[11],
        INHABITED_TIME[12],
        0,
        0,
        0,
    ]);

    chunk2_buffer == chunk2_target
}

fn search_compound_recursive<R: Read>(
    reader: &mut R,
    max_depth: u8,
    current_depth: u8,
) -> Result<Option<i64>, NbtError> {
    loop {
        let tag_type = reader.read_u8()?;
        if tag_type == TAG_END {
            return Ok(None);
        }

        // Check if this is "InhabitedTime" without allocating
        let is_inhabited_time = is_inhabited_time_string(reader)?;

        if !is_inhabited_time {
            // For compounds, recurse if within depth limit - this is less common
            if tag_type == TAG_COMPOUND && current_depth < max_depth {
                if let Some(result) =
                    search_compound_recursive(reader, max_depth, current_depth + 1)?
                {
                    return Ok(Some(result));
                }
            } else {
                skip_tag_value(reader, tag_type)?;
            }
        } else {
            return match tag_type {
                TAG_LONG => Ok(Some(reader.read_i64::<BigEndian>()?)),
                TAG_INT => Ok(Some(reader.read_i32::<BigEndian>()? as i64)),
                TAG_SHORT => Ok(Some(reader.read_i16::<BigEndian>()? as i64)),
                TAG_BYTE => Ok(Some(reader.read_i8()? as i64)),
                _ => Err(NbtError::InvalidFormat(format!(
                    "InhabitedTime has unexpected type: {}",
                    tag_type
                ))),
            };
        }
    }
}

fn skip_string<R: Read>(reader: &mut R) -> Result<(), NbtError> {
    let length = reader.read_u16::<BigEndian>()? as usize;
    skip_bytes(reader, length)
}

#[inline(always)]
fn skip_bytes<R: Read>(reader: &mut R, count: usize) -> Result<(), NbtError> {
    // For very small skips, read directly into a stack buffer to avoid overhead
    if count <= 64 {
        let mut buffer = [0u8; 64];
        reader.read_exact(&mut buffer[..count])?;
    } else if count <= 1024 {
        // For medium skips, use a larger stack buffer
        let mut buffer = [0u8; 1024];
        reader.read_exact(&mut buffer[..count])?;
    } else {
        // For large skips, use the standard approach
        std::io::copy(&mut reader.take(count as u64), &mut std::io::sink())?;
    }
    Ok(())
}

fn skip_tag_value<R: Read>(reader: &mut R, tag_type: u8) -> Result<(), NbtError> {
    match tag_type {
        TAG_BYTE => {
            reader.read_u8()?;
        }
        TAG_SHORT => {
            reader.read_u16::<BigEndian>()?;
        }
        TAG_INT => {
            reader.read_u32::<BigEndian>()?;
        }
        TAG_LONG => {
            reader.read_u64::<BigEndian>()?;
        }
        TAG_FLOAT => {
            reader.read_f32::<BigEndian>()?;
        }
        TAG_DOUBLE => {
            reader.read_f64::<BigEndian>()?;
        }
        TAG_BYTE_ARRAY => {
            let length = reader.read_i32::<BigEndian>()? as usize;
            skip_bytes(reader, length)?;
        }
        TAG_STRING => {
            skip_string(reader)?;
        }
        TAG_LIST => {
            let list_type = reader.read_u8()?;
            let length = reader.read_i32::<BigEndian>()? as usize;
            match list_type {
                TAG_BYTE => skip_bytes(reader, length)?,
                TAG_SHORT => skip_bytes(reader, length * 2)?,
                TAG_INT => skip_bytes(reader, length * 4)?,
                TAG_LONG => skip_bytes(reader, length * 8)?,
                TAG_FLOAT => skip_bytes(reader, length * 4)?,
                TAG_DOUBLE => skip_bytes(reader, length * 8)?,
                // Less common cases
                _ => {
                    if length > 0 {
                        for _ in 0..length {
                            skip_tag_value(reader, list_type)?;
                        }
                    }
                }
            }
        }
        TAG_COMPOUND => {
            skip_compound(reader)?;
        }
        TAG_INT_ARRAY => {
            let length = reader.read_i32::<BigEndian>()? as usize;
            skip_bytes(reader, length * 4)?;
        }
        TAG_LONG_ARRAY => {
            let length = reader.read_i32::<BigEndian>()? as usize;
            skip_bytes(reader, length * 8)?;
        }
        _ => {
            return Err(NbtError::InvalidFormat(format!(
                "Unknown tag type: {}",
                tag_type
            )));
        }
    }

    Ok(())
}

fn skip_compound<R: Read>(reader: &mut R) -> Result<(), NbtError> {
    loop {
        let tag_type = reader.read_u8()?;
        if tag_type == TAG_END {
            break;
        }

        skip_string(reader)?;
        skip_tag_value(reader, tag_type)?;
    }

    Ok(())
}

pub fn process_chunk(chunk_data: &[u8]) -> Result<Option<i64>, ProcessError> {
    extract_inhabited_time(chunk_data)
        .map_err(|e| ProcessError::ChunkError(format!("Fast NBT parsing failed: {}", e)))
}
