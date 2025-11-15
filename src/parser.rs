use std::io::ErrorKind;

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

    let mut p = chunk_data;

    // Read root compound tag
    let tag_type = read_u8(&mut p)?;
    if tag_type != TAG_COMPOUND {
        return Err(NbtError::InvalidFormat(
            "Root tag is not a compound".to_string(),
        ));
    }

    // Skip root tag name
    skip_string(&mut p)?;

    // Search through the root compound for InhabitedTime
    search_compound(&mut p, 2)
}

fn search_compound(reader: &mut &[u8], max_depth: u8) -> Result<Option<i64>, NbtError> {
    search_compound_recursive(reader, max_depth, 0)
}

/// Check if a string matches "InhabitedTime" without allocating
#[inline(always)]
fn is_inhabited_time_string(reader: &mut &[u8]) -> Result<bool, NbtError> {
    let length = read_u16_be(reader)? as usize;

    const INHABITED_TIME: &[u8] = b"InhabitedTime";

    if length != INHABITED_TIME.len() {
        skip_bytes(reader, length)?;
        return Ok(false);
    }

    // Fast path: single bounds check then two u64 comparisons (13 bytes)
    ensure_len(reader, INHABITED_TIME.len())?;
    let b = *reader;

    // First 8 bytes
    let chunk1 = u64::from_ne_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]);
    let target1 = u64::from_ne_bytes([
        INHABITED_TIME[0],
        INHABITED_TIME[1],
        INHABITED_TIME[2],
        INHABITED_TIME[3],
        INHABITED_TIME[4],
        INHABITED_TIME[5],
        INHABITED_TIME[6],
        INHABITED_TIME[7],
    ]);
    if chunk1 != target1 {
        // advance past the name
        *reader = &b[INHABITED_TIME.len()..];
        return Ok(false);
    }

    // Remaining 5 bytes, compare in a u64 with padding
    let chunk2 = u64::from_ne_bytes([
        b[8], b[9], b[10], b[11], b[12], 0, 0, 0,
    ]);
    let target2 = u64::from_ne_bytes([
        INHABITED_TIME[8],
        INHABITED_TIME[9],
        INHABITED_TIME[10],
        INHABITED_TIME[11],
        INHABITED_TIME[12],
        0,
        0,
        0,
    ]);

    let res = chunk2 == target2;
    *reader = &b[INHABITED_TIME.len()..];
    Ok(res)
}

fn search_compound_recursive(
    reader: &mut &[u8],
    max_depth: u8,
    current_depth: u8,
) -> Result<Option<i64>, NbtError> {
    loop {
        let tag_type = read_u8(reader)?;
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
                TAG_LONG => Ok(Some(read_i64_be(reader)?)),
                TAG_INT => Ok(Some(read_i32_be(reader)? as i64)),
                TAG_SHORT => Ok(Some(read_i16_be(reader)? as i64)),
                TAG_BYTE => Ok(Some(read_i8(reader)? as i64)),
                _ => Err(NbtError::InvalidFormat(format!(
                    "InhabitedTime has unexpected type: {}",
                    tag_type
                ))),
            };
        }
    }
}

fn skip_string(reader: &mut &[u8]) -> Result<(), NbtError> {
    let length = read_u16_be(reader)? as usize;
    skip_bytes(reader, length)
}

#[inline(always)]
fn skip_bytes(reader: &mut &[u8], count: usize) -> Result<(), NbtError> {
    let len = reader.len();
    if len < count {
        return Err(NbtError::InvalidFormat("Unexpected EOF".into()));
    }
    unsafe {
        *reader = reader.get_unchecked(count..);
    }
    Ok(())
}


fn skip_tag_value(reader: &mut &[u8], tag_type: u8) -> Result<(), NbtError> {
    match tag_type {
        TAG_BYTE => {
            read_u8(reader)?;
        }
        TAG_SHORT => {
            read_u16_be(reader)?;
        }
        TAG_INT => {
            read_u32_be(reader)?;
        }
        TAG_LONG => {
            read_u64_be(reader)?;
        }
        TAG_FLOAT => {
            read_f32_be(reader)?;
        }
        TAG_DOUBLE => {
            read_f64_be(reader)?;
        }
        TAG_BYTE_ARRAY => {
            let length = read_i32_be(reader)? as usize;
            skip_bytes(reader, length)?;
        }
        TAG_STRING => {
            skip_string(reader)?;
        }
        TAG_LIST => {
            let list_type = read_u8(reader)?;
            let length = read_i32_be(reader)? as usize;
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
            let length = read_i32_be(reader)? as usize;
            skip_bytes(reader, length * 4)?;
        }
        TAG_LONG_ARRAY => {
            let length = read_i32_be(reader)? as usize;
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

fn skip_compound(reader: &mut &[u8]) -> Result<(), NbtError> {
    loop {
        let tag_type = read_u8(reader)?;
        if tag_type == TAG_END {
            break;
        }

        skip_string(reader)?;
        skip_tag_value(reader, tag_type)?;
    }

    Ok(())
}

#[inline(always)]
fn ensure_len(p: &mut &[u8], need: usize) -> Result<(), NbtError> {
    if p.len() < need {
        return Err(NbtError::IoError(std::io::Error::new(
            ErrorKind::UnexpectedEof,
            "unexpected EOF while parsing NBT",
        )));
    }
    Ok(())
}

#[inline(always)]
fn read_u8(p: &mut &[u8]) -> Result<u8, NbtError> {
    ensure_len(p, 1)?;
    let v = p[0];
    *p = &p[1..];
    Ok(v)
}

#[inline(always)]
fn read_i8(p: &mut &[u8]) -> Result<i8, NbtError> {
    Ok(read_u8(p)? as i8)
}

#[inline(always)]
fn read_u16_be(p: &mut &[u8]) -> Result<u16, NbtError> {
    ensure_len(p, 2)?;
    let v = u16::from_be_bytes([p[0], p[1]]);
    *p = &p[2..];
    Ok(v)
}

#[inline(always)]
fn read_i16_be(p: &mut &[u8]) -> Result<i16, NbtError> {
    Ok(read_u16_be(p)? as i16)
}

#[inline(always)]
fn read_u32_be(p: &mut &[u8]) -> Result<u32, NbtError> {
    ensure_len(p, 4)?;
    let v = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
    *p = &p[4..];
    Ok(v)
}

#[inline(always)]
fn read_i32_be(p: &mut &[u8]) -> Result<i32, NbtError> {
    Ok(read_u32_be(p)? as i32)
}

#[inline(always)]
fn read_u64_be(p: &mut &[u8]) -> Result<u64, NbtError> {
    ensure_len(p, 8)?;
    let v = u64::from_be_bytes([
        p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7],
    ]);
    *p = &p[8..];
    Ok(v)
}

#[inline(always)]
fn read_i64_be(p: &mut &[u8]) -> Result<i64, NbtError> {
    Ok(read_u64_be(p)? as i64)
}

#[inline(always)]
fn read_f32_be(p: &mut &[u8]) -> Result<f32, NbtError> {
    let bits = read_u32_be(p)?;
    Ok(f32::from_bits(bits))
}

#[inline(always)]
fn read_f64_be(p: &mut &[u8]) -> Result<f64, NbtError> {
    let bits = read_u64_be(p)?;
    Ok(f64::from_bits(bits))
}


pub fn process_chunk(chunk_data: &[u8]) -> Result<Option<i64>, ProcessError> {
    extract_inhabited_time(chunk_data)
        .map_err(|e| ProcessError::ChunkError(format!("Fast NBT parsing failed: {}", e)))
}
