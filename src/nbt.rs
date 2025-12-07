use std::io::ErrorKind;

/// Branch prediction hints for hot paths
#[inline(always)]
#[cold]
const fn cold() {}

#[inline(always)]
const fn likely(b: bool) -> bool {
    if !b {
        cold();
    }
    b
}

#[inline(always)]
const fn unlikely(b: bool) -> bool {
    if b {
        cold();
    }
    b
}

// Const error messages to avoid allocations in hot paths
pub const ERR_NEGATIVE_ARRAY: &str = "Negative array length";
pub const ERR_NEGATIVE_LIST: &str = "Negative list length";
pub const ERR_SIZE_OVERFLOW: &str = "Size overflow";
pub const ERR_UNEXPECTED_TYPE: &str = "Field has unexpected type";
pub const ERR_NOT_COMPOUND: &str = "Root tag is not a compound";
pub const ERR_UNKNOWN_TAG: &str = "Unknown NBT tag type";
pub const ERR_NOT_FOUND: &str = "Field not found";
const ERR_UNEXPECTED_EOF: &str = "unexpected EOF while parsing NBT";

#[derive(Debug)]
pub enum NbtError {
    IoError(std::io::Error),
    InvalidFormat(String),
    FieldNotFound(Vec<u8>),
}

impl From<std::io::Error> for NbtError {
    #[inline]
    fn from(error: std::io::Error) -> Self {
        Self::IoError(error)
    }
}

impl std::fmt::Display for NbtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "IO error: {e}"),
            Self::InvalidFormat(msg) => write!(f, "Invalid NBT format: {msg}"),
            Self::FieldNotFound(field) => {
                write!(f, "Field not found: {}", String::from_utf8_lossy(field))
            }
        }
    }
}
impl std::error::Error for NbtError {}

/// Helper to generate cold EOF error
#[inline(always)]
#[cold]
fn eof_error() -> NbtError {
    NbtError::IoError(std::io::Error::new(
        ErrorKind::UnexpectedEof,
        ERR_UNEXPECTED_EOF,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NbtTag {
    End = 0,
    Byte = 1,
    Short = 2,
    Int = 3,
    Long = 4,
    Float = 5,
    Double = 6,
    ByteArray = 7,
    String = 8,
    List = 9,
    Compound = 10,
    IntArray = 11,
    LongArray = 12,
}

#[derive(Debug, Clone, PartialEq)]
#[repr(u8)]
pub enum NbtValue {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
}

const TAG_LOOKUP: [Option<NbtTag>; 256] = {
    let mut table = [None; 256];
    table[0] = Some(NbtTag::End);
    table[1] = Some(NbtTag::Byte);
    table[2] = Some(NbtTag::Short);
    table[3] = Some(NbtTag::Int);
    table[4] = Some(NbtTag::Long);
    table[5] = Some(NbtTag::Float);
    table[6] = Some(NbtTag::Double);
    table[7] = Some(NbtTag::ByteArray);
    table[8] = Some(NbtTag::String);
    table[9] = Some(NbtTag::List);
    table[10] = Some(NbtTag::Compound);
    table[11] = Some(NbtTag::IntArray);
    table[12] = Some(NbtTag::LongArray);
    table
};

impl NbtTag {
    #[inline]
    pub fn from_u8(value: u8) -> Result<Self, NbtError> {
        TAG_LOOKUP[value as usize].ok_or_else(|| NbtError::InvalidFormat(ERR_UNKNOWN_TAG.into()))
    }
}
pub struct NbtReader<'a> {
    data: &'a [u8],
}

impl<'a> NbtReader<'a> {
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    #[inline]
    #[must_use]
    pub const fn position(&self, original_data: &'a [u8]) -> usize {
        original_data.len() - self.data.len()
    }

    #[inline(always)]
    pub fn read_u8(&mut self) -> Result<u8, NbtError> {
        if likely(!self.data.is_empty()) {
            let byte = unsafe { *self.data.get_unchecked(0) };
            self.data = unsafe { self.data.get_unchecked(1..) };
            Ok(byte)
        } else {
            Err(eof_error())
        }
    }

    #[inline]
    pub fn read_i8(&mut self) -> Result<i8, NbtError> {
        self.read_u8().map(u8::cast_signed)
    }

    #[inline(always)]
    pub fn read_u16_be(&mut self) -> Result<u16, NbtError> {
        if likely(self.data.len() >= 2) {
            // Use unaligned read for better performance on modern CPUs
            let v =
                unsafe { u16::from_be(std::ptr::read_unaligned(self.data.as_ptr().cast::<u16>())) };
            self.data = unsafe { self.data.get_unchecked(2..) };
            Ok(v)
        } else {
            Err(eof_error())
        }
    }

    #[inline]
    pub fn read_i16_be(&mut self) -> Result<i16, NbtError> {
        self.read_u16_be().map(u16::cast_signed)
    }

    #[inline]
    pub fn read_u32_be(&mut self) -> Result<u32, NbtError> {
        if self.data.len() >= 4 {
            let v = u32::from_be_bytes(unsafe {
                [
                    *self.data.get_unchecked(0),
                    *self.data.get_unchecked(1),
                    *self.data.get_unchecked(2),
                    *self.data.get_unchecked(3),
                ]
            });
            self.data = unsafe { self.data.get_unchecked(4..) };
            Ok(v)
        } else {
            Err(eof_error())
        }
    }

    #[inline]
    pub fn read_i32_be(&mut self) -> Result<i32, NbtError> {
        self.read_u32_be().map(u32::cast_signed)
    }

    #[inline]
    pub fn read_u64_be(&mut self) -> Result<u64, NbtError> {
        if self.data.len() >= 8 {
            let v = u64::from_be_bytes(unsafe {
                [
                    *self.data.get_unchecked(0),
                    *self.data.get_unchecked(1),
                    *self.data.get_unchecked(2),
                    *self.data.get_unchecked(3),
                    *self.data.get_unchecked(4),
                    *self.data.get_unchecked(5),
                    *self.data.get_unchecked(6),
                    *self.data.get_unchecked(7),
                ]
            });
            self.data = unsafe { self.data.get_unchecked(8..) };
            Ok(v)
        } else {
            Err(eof_error())
        }
    }

    #[inline]
    pub fn read_i64_be(&mut self) -> Result<i64, NbtError> {
        self.read_u64_be().map(u64::cast_signed)
    }

    #[inline]
    pub fn read_f32_be(&mut self) -> Result<f32, NbtError> {
        self.read_u32_be().map(f32::from_bits)
    }

    #[inline]
    pub fn read_f64_be(&mut self) -> Result<f64, NbtError> {
        self.read_u64_be().map(f64::from_bits)
    }

    #[inline]
    pub fn read_string(&mut self) -> Result<String, NbtError> {
        let length = self.read_u16_be()? as usize;
        let string = unsafe { self.data.get_unchecked(..length) };
        self.data = unsafe { self.data.get_unchecked(length..) };
        Ok(String::from_utf8_lossy(string).into_owned())
    }

    #[inline(always)]
    pub fn skip_bytes(&mut self, count: usize) -> Result<(), NbtError> {
        if likely(self.data.len() >= count) {
            self.data = unsafe { self.data.get_unchecked(count..) };
            Ok(())
        } else {
            Err(eof_error())
        }
    }

    #[inline(always)]
    pub fn skip_string(&mut self) -> Result<(), NbtError> {
        let length = self.read_u16_be()? as usize;
        self.skip_bytes(length)
    }

    fn skip_list_items(&mut self, list_type: NbtTag, count: usize) -> Result<(), NbtError> {
        match list_type {
            NbtTag::Compound => {
                for _ in 0..count {
                    self.skip_compound()?;
                }
            }
            NbtTag::String => {
                for _ in 0..count {
                    self.skip_string()?;
                }
            }
            NbtTag::List => {
                for _ in 0..count {
                    // Recursively skip lists
                    // We must read the header for each inner list
                    let inner_type = NbtTag::from_u8(self.read_u8()?)?;
                    let inner_len: usize = self
                        .read_i32_be()?
                        .try_into()
                        .map_err(|_| NbtError::InvalidFormat(ERR_NEGATIVE_LIST.into()))?;
                    // Recursively call the optimized list skipper
                    self.handle_sized_skip(inner_type, inner_len)?;
                }
            }
            // For fixed-size primitives, the caller (handle_sized_skip) usually
            // handles them via multiplication, but if we fall through here (e.g. List<ByteArray>),
            // we use the generic skipper.
            _ => {
                for _ in 0..count {
                    self.skip_tag_value(list_type)?;
                }
            }
        }
        Ok(())
    }

    /// Unified logic for skipping a sequence of N items of Type T.
    /// Used by both `skip_tag_value` (for Arrays/Lists) and `skip_list_items` (recursion).
    #[inline]
    fn handle_sized_skip(&mut self, tag_type: NbtTag, length: usize) -> Result<(), NbtError> {
        match tag_type {
            // Primitives: O(1) skip using math
            NbtTag::Byte => self.skip_bytes(length),
            NbtTag::Short => self.skip_bytes(
                length
                    .checked_mul(2)
                    .ok_or_else(|| NbtError::InvalidFormat(ERR_SIZE_OVERFLOW.into()))?,
            ),
            NbtTag::Int | NbtTag::Float => self.skip_bytes(
                length
                    .checked_mul(4)
                    .ok_or_else(|| NbtError::InvalidFormat(ERR_SIZE_OVERFLOW.into()))?,
            ),
            NbtTag::Long | NbtTag::Double => self.skip_bytes(
                length
                    .checked_mul(8)
                    .ok_or_else(|| NbtError::InvalidFormat(ERR_SIZE_OVERFLOW.into()))?,
            ),

            // Complex types: O(N) loop
            _ => self.skip_list_items(tag_type, length),
        }
    }

    #[inline]
    pub fn skip_tag_value(&mut self, tag_type: NbtTag) -> Result<(), NbtError> {
        match tag_type {
            NbtTag::Byte => self.skip_bytes(1),
            NbtTag::Short => self.skip_bytes(2),
            NbtTag::Int | NbtTag::Float => self.skip_bytes(4),
            NbtTag::Long | NbtTag::Double => self.skip_bytes(8),
            NbtTag::String => self.skip_string(),
            NbtTag::ByteArray => {
                let length: usize = self
                    .read_i32_be()?
                    .try_into()
                    .map_err(|_| NbtError::InvalidFormat(ERR_NEGATIVE_ARRAY.into()))?;
                self.skip_bytes(length)
            }
            NbtTag::IntArray => {
                let length: usize = self
                    .read_i32_be()?
                    .try_into()
                    .map_err(|_| NbtError::InvalidFormat(ERR_NEGATIVE_ARRAY.into()))?;
                self.handle_sized_skip(NbtTag::Int, length)
            }
            NbtTag::LongArray => {
                let length: usize = self
                    .read_i32_be()?
                    .try_into()
                    .map_err(|_| NbtError::InvalidFormat(ERR_NEGATIVE_ARRAY.into()))?;
                self.handle_sized_skip(NbtTag::Long, length)
            }
            NbtTag::List => {
                let list_type = NbtTag::from_u8(self.read_u8()?)?;
                let length: usize = self
                    .read_i32_be()?
                    .try_into()
                    .map_err(|_| NbtError::InvalidFormat(ERR_NEGATIVE_LIST.into()))?;
                self.handle_sized_skip(list_type, length)
            }
            NbtTag::Compound => self.skip_compound(),
            NbtTag::End => Ok(()),
        }
    }

    #[inline]
    pub fn skip_compound(&mut self) -> Result<(), NbtError> {
        loop {
            // Prefetch ahead for better cache performance
            #[cfg(target_arch = "x86_64")]
            if self.data.len() >= 128 {
                unsafe {
                    std::arch::x86_64::_mm_prefetch::<{ std::arch::x86_64::_MM_HINT_T0 }>(
                        self.data.as_ptr().add(64).cast::<i8>(),
                    );
                }
            }

            let tag_byte = self.read_u8()?;
            // End tag is rare in the middle of compound, common at the end
            if unlikely(tag_byte == 0) {
                break;
            }
            let tag_type = NbtTag::from_u8(tag_byte)?;

            self.skip_string()?;
            self.skip_tag_value(tag_type)?;
        }
        Ok(())
    }

    // Tries to search for the field, if it's found, then it returns the value and the byte position where it was found
    #[inline]
    pub fn search_compound_for_field(
        &mut self,
        field_name: &[u8],
        original_data: &'a [u8],
    ) -> Result<(Option<NbtValue>, usize), NbtError> {
        let field_name_len = field_name.len();

        loop {
            let tag_type = NbtTag::from_u8(self.read_u8()?)?;
            if tag_type == NbtTag::End {
                return Err(NbtError::FieldNotFound(field_name.to_vec()));
            }

            let name_length = self.read_u16_be()? as usize;

            // If lengths don't match, don't bother comparing bytes
            let is_match = if name_length == field_name_len {
                if self.data.len() < name_length {
                    return Err(eof_error());
                }
                let m = unsafe { self.data.get_unchecked(..name_length) == field_name };
                self.data = unsafe { self.data.get_unchecked(name_length..) };
                m
            } else {
                self.skip_bytes(name_length)?;
                false
            };

            if is_match {
                let position = self.position(original_data); // Position *at start of value*
                let val = match tag_type {
                    NbtTag::Long => NbtValue::Long(self.read_i64_be()?),
                    NbtTag::Int => NbtValue::Int(self.read_i32_be()?),
                    NbtTag::Short => NbtValue::Short(self.read_i16_be()?),
                    NbtTag::Byte => NbtValue::Byte(self.read_i8()?),
                    NbtTag::Double => NbtValue::Double(self.read_f64_be()?),
                    NbtTag::Float => NbtValue::Float(self.read_f32_be()?),
                    NbtTag::String => NbtValue::String(self.read_string()?),
                    _ => unimplemented!(),
                };
                return Ok((Some(val), position));
            }
            self.skip_tag_value(tag_type)?;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeResult {
    pub time: Option<i64>, // InhabitedTime in ticks
}

/// Gets the `InhabitedTime` field from chunk NBT data.
///
/// # Arguments
/// * `chunk_data` - Raw NBT data from a Minecraft chunk
///
#[inline]
pub fn get_inhabited_time(chunk_data: &[u8]) -> Result<TimeResult, NbtError> {
    const INHABITED_TIME: &[u8] = b"InhabitedTime";

    // Prefetch the beginning of the chunk data into cache
    if chunk_data.len() >= 64 {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            if is_x86_feature_detected!("sse") {
                std::arch::x86_64::_mm_prefetch(
                    chunk_data.as_ptr().cast::<i8>(),
                    std::arch::x86_64::_MM_HINT_T0,
                );
            }
        }
    }

    let mut reader = NbtReader::new(chunk_data);

    // Read root compound tag
    let tag_type = NbtTag::from_u8(reader.read_u8()?)?;
    if tag_type != NbtTag::Compound {
        return Err(NbtError::InvalidFormat(ERR_NOT_COMPOUND.into()));
    }

    // Skip root tag name
    reader.skip_string()?;

    // Search through the root compound for InhabitedTime
    let (time, byte_pos) = reader.search_compound_for_field(INHABITED_TIME, chunk_data)?;
    if let Some(NbtValue::Long(time)) = time {
        if byte_pos > 0 {
            Ok(TimeResult { time: Some(time) })
        } else {
            Err(NbtError::InvalidFormat(ERR_NOT_FOUND.into()))
        }
    } else {
        Err(NbtError::InvalidFormat(ERR_NOT_FOUND.into()))
    }
}
