use std::io::ErrorKind;

/// NBT parsing errors.
#[derive(Debug)]
pub enum NbtError {
    /// I/O error occurred during parsing
    IoError(std::io::Error),
    /// Invalid or malformed NBT format
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

/// NBT Tag types as defined in the NBT specification.
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

impl NbtTag {
    /// Convert a byte value to an NbtTag.
    #[inline(always)]
    pub fn from_u8(value: u8) -> Result<Self, NbtError> {
        match value {
            0 => Ok(NbtTag::End),
            1 => Ok(NbtTag::Byte),
            2 => Ok(NbtTag::Short),
            3 => Ok(NbtTag::Int),
            4 => Ok(NbtTag::Long),
            5 => Ok(NbtTag::Float),
            6 => Ok(NbtTag::Double),
            7 => Ok(NbtTag::ByteArray),
            8 => Ok(NbtTag::String),
            9 => Ok(NbtTag::List),
            10 => Ok(NbtTag::Compound),
            11 => Ok(NbtTag::IntArray),
            12 => Ok(NbtTag::LongArray),
            _ => Err(NbtError::InvalidFormat(format!(
                "Unknown NBT tag type: {}",
                value
            ))),
        }
    }

    /// Convert the NbtTag to its byte representation.
    #[inline(always)]
    #[allow(dead_code)]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// A zero-copy NBT reader that operates on a borrowed byte slice.
/// Provides efficient reading and skipping operations for NBT data.
pub struct NbtReader<'a> {
    /// The data slice to read from
    data: &'a [u8],
}

impl<'a> NbtReader<'a> {
    /// Create a new NbtReader from a byte slice.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Get the current byte position in the original data slice.
    /// Useful for tracking how far into the data we've read.
    #[inline(always)]
    pub fn position(&self, original_data: &'a [u8]) -> usize {
        original_data.len() - self.data.len()
    }

    /// Ensure at least `need` bytes are available.
    #[inline(always)]
    fn ensure_len(&self, need: usize) -> Result<(), NbtError> {
        if self.data.len() < need {
            return Err(NbtError::IoError(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "unexpected EOF while parsing NBT",
            )));
        }
        Ok(())
    }

    /// Read a single unsigned byte and advance the reader.
    #[inline(always)]
    pub fn read_u8(&mut self) -> Result<u8, NbtError> {
        self.ensure_len(1)?;
        let v = self.data[0];
        self.data = &self.data[1..];
        Ok(v)
    }

    /// Read a single signed byte and advance the reader.
    #[inline(always)]
    pub fn read_i8(&mut self) -> Result<i8, NbtError> {
        Ok(self.read_u8()? as i8)
    }

    /// Read an unsigned 16-bit big-endian integer and advance the reader.
    #[inline(always)]
    pub fn read_u16_be(&mut self) -> Result<u16, NbtError> {
        self.ensure_len(2)?;
        let v = u16::from_be_bytes([self.data[0], self.data[1]]);
        self.data = &self.data[2..];
        Ok(v)
    }

    /// Read a signed 16-bit big-endian integer and advance the reader.
    #[inline(always)]
    pub fn read_i16_be(&mut self) -> Result<i16, NbtError> {
        Ok(self.read_u16_be()? as i16)
    }

    /// Read an unsigned 32-bit big-endian integer and advance the reader.
    #[inline(always)]
    pub fn read_u32_be(&mut self) -> Result<u32, NbtError> {
        self.ensure_len(4)?;
        let v = u32::from_be_bytes([self.data[0], self.data[1], self.data[2], self.data[3]]);
        self.data = &self.data[4..];
        Ok(v)
    }

    /// Read a signed 32-bit big-endian integer and advance the reader.
    #[inline(always)]
    pub fn read_i32_be(&mut self) -> Result<i32, NbtError> {
        Ok(self.read_u32_be()? as i32)
    }

    /// Read an unsigned 64-bit big-endian integer and advance the reader.
    #[inline(always)]
    pub fn read_u64_be(&mut self) -> Result<u64, NbtError> {
        self.ensure_len(8)?;
        let v = u64::from_be_bytes([
            self.data[0],
            self.data[1],
            self.data[2],
            self.data[3],
            self.data[4],
            self.data[5],
            self.data[6],
            self.data[7],
        ]);
        self.data = &self.data[8..];
        Ok(v)
    }

    /// Read a signed 64-bit big-endian integer and advance the reader.
    #[inline(always)]
    pub fn read_i64_be(&mut self) -> Result<i64, NbtError> {
        Ok(self.read_u64_be()? as i64)
    }

    /// Read a 32-bit big-endian float and advance the reader.
    #[inline(always)]
    pub fn read_f32_be(&mut self) -> Result<f32, NbtError> {
        let bits = self.read_u32_be()?;
        Ok(f32::from_bits(bits))
    }

    /// Read a 64-bit big-endian double and advance the reader.
    #[inline(always)]
    pub fn read_f64_be(&mut self) -> Result<f64, NbtError> {
        let bits = self.read_u64_be()?;
        Ok(f64::from_bits(bits))
    }

    /// Skip a specified number of bytes.
    /// Uses unsafe for performance - bounds checking is done once.
    #[inline(always)]
    pub fn skip_bytes(&mut self, count: usize) -> Result<(), NbtError> {
        if self.data.len() < count {
            return Err(NbtError::IoError(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "Unexpected EOF",
            )));
        }
        unsafe {
            self.data = self.data.get_unchecked(count..);
        }
        Ok(())
    }

    /// Skip over a string value (length-prefixed).
    pub fn skip_string(&mut self) -> Result<(), NbtError> {
        let length = self.read_u16_be()? as usize;
        self.skip_bytes(length)
    }

    /// Skip over a tag value based on its type.
    pub fn skip_tag_value(&mut self, tag_type: NbtTag) -> Result<(), NbtError> {
        match tag_type {
            NbtTag::Byte => {
                self.skip_bytes(1)?;
            }
            NbtTag::Short => {
                self.skip_bytes(2)?;
            }
            NbtTag::Int | NbtTag::Float => {
                self.skip_bytes(4)?;
            }
            NbtTag::Long | NbtTag::Double => {
                self.skip_bytes(8)?;
            }
            NbtTag::ByteArray => {
                let length = self.read_i32_be()?;
                if length < 0 {
                    return Err(NbtError::InvalidFormat(format!(
                        "Negative array length: {}",
                        length
                    )));
                }
                self.skip_bytes(length as usize)?;
            }
            NbtTag::String => {
                self.skip_string()?;
            }
            NbtTag::List => {
                let list_type = NbtTag::from_u8(self.read_u8()?)?;
                let length = self.read_i32_be()?;
                if length < 0 {
                    return Err(NbtError::InvalidFormat(format!(
                        "Negative list length: {}",
                        length
                    )));
                }
                let length = length as usize;
                
                match list_type {
                    NbtTag::Byte => self.skip_bytes(length)?,
                    NbtTag::Short => self.skip_bytes(
                        length
                            .checked_mul(2)
                            .ok_or(NbtError::InvalidFormat("List size overflow".into()))?,
                    )?,
                    NbtTag::Int | NbtTag::Float => self.skip_bytes(
                        length
                            .checked_mul(4)
                            .ok_or(NbtError::InvalidFormat("List size overflow".into()))?,
                    )?,
                    NbtTag::Long | NbtTag::Double => self.skip_bytes(
                        length
                            .checked_mul(8)
                            .ok_or(NbtError::InvalidFormat("List size overflow".into()))?,
                    )?,
                    // Complex types need individual skipping
                    _ => {
                        for _ in 0..length {
                            self.skip_tag_value(list_type)?;
                        }
                    }
                }
            }
            NbtTag::Compound => {
                self.skip_compound()?;
            }
            NbtTag::IntArray => {
                let length = self.read_i32_be()?;
                if length < 0 {
                    return Err(NbtError::InvalidFormat(format!(
                        "Negative array length: {}",
                        length
                    )));
                }
                self.skip_bytes(
                    (length as usize)
                        .checked_mul(4)
                        .ok_or(NbtError::InvalidFormat("Array size overflow".into()))?,
                )?;
            }
            NbtTag::LongArray => {
                let length = self.read_i32_be()?;
                if length < 0 {
                    return Err(NbtError::InvalidFormat(format!(
                        "Negative array length: {}",
                        length
                    )));
                }
                self.skip_bytes(
                    (length as usize)
                        .checked_mul(8)
                        .ok_or(NbtError::InvalidFormat("Array size overflow".into()))?,
                )?;
            }
            NbtTag::End => {
                // TAG_END has no data
            }
        }

        Ok(())
    }

    /// Skip over an entire compound tag (including nested compounds).
    pub fn skip_compound(&mut self) -> Result<(), NbtError> {
        loop {
            let tag_type = NbtTag::from_u8(self.read_u8()?)?;
            if tag_type == NbtTag::End {
                break;
            }

            self.skip_string()?;
            self.skip_tag_value(tag_type)?;
        }

        Ok(())
    }

    /// Check if the next string matches a target string without allocating.
    /// Uses byte slice comparison for matching.
    /// Returns true if the string matches and advances the reader past it.
    ///
    /// # Arguments
    /// * `target` - The target string to match against
    #[inline(always)]
    pub fn is_string_match(&mut self, target: &[u8]) -> Result<bool, NbtError> {
        let length = self.read_u16_be()? as usize;

        if length != target.len() {
            self.skip_bytes(length)?;
            return Ok(false);
        }

        self.ensure_len(length)?;
        
        let matches = &self.data[..length] == target;
        self.data = &self.data[length..];
        Ok(matches)
    }

    /// Search through a compound tag for a specific field at the current depth only.
    /// Does not recurse into nested compounds.
    ///
    /// # Arguments
    /// * `field_name` - The name of the field to search for
    /// * `original_data` - The original data slice (for position tracking)
    ///
    /// # Returns
    /// A tuple of (Option<i64>, usize) where:
    /// - First element is the field value if found
    /// - Second element is the byte position where the value data starts in the original buffer (0 if not found)
    ///   This position points to the beginning of the value bytes, before they are read.
    pub fn search_compound_for_field(
        &mut self,
        field_name: &[u8],
        original_data: &'a [u8],
    ) -> Result<(Option<i64>, usize), NbtError> {
        loop {
            let tag_type = NbtTag::from_u8(self.read_u8()?)?;
            if tag_type == NbtTag::End {
                return Ok((None, 0));
            }

            // Check if this field matches our target
            let is_target_field = self.is_string_match(field_name)?;

            if !is_target_field {
                // Skip this tag's value
                self.skip_tag_value(tag_type)?;
            } else {
                // Found the target field, record position before reading value
                let position = self.position(original_data);

                // Read its value
                let result = match tag_type {
                    NbtTag::Long => Ok((Some(self.read_i64_be()?), position)),
                    NbtTag::Int => Ok((Some(self.read_i32_be()? as i64), position)),
                    NbtTag::Short => Ok((Some(self.read_i16_be()? as i64), position)),
                    NbtTag::Byte => Ok((Some(self.read_i8()? as i64), position)),
                    _ => Err(NbtError::InvalidFormat(format!(
                        "Field has unexpected type: {:?}",
                        tag_type
                    ))),
                };

                return result;
            }
        }
    }
}
