//! Reading Unreal's little-endian serialization from bytes. Files are untrusted: every read is
//! checked against the end, and counts are checked before anything is allocated for them.

/// A position in bytes being read.
pub struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    /// Reading `data` from `at`.
    pub fn at(data: &'a [u8], at: usize) -> Result<Self, String> {
        if at > data.len() {
            return Err(format!("offset {at} past the end ({} bytes)", data.len()));
        }
        Ok(Self { data, at })
    }

    pub fn position(&self) -> usize {
        self.at
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(count)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| {
                format!(
                    "unexpected end of data (wanted {count} bytes at {})",
                    self.at
                )
            })?;
        let taken = &self.data[self.at..end];
        self.at = end;
        Ok(taken)
    }

    pub fn skip(&mut self, count: usize) -> Result<(), String> {
        self.take(count).map(|_| ())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        self.array().map(u16::from_le_bytes)
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        self.array().map(u32::from_le_bytes)
    }

    pub fn i32(&mut self) -> Result<i32, String> {
        self.array().map(i32::from_le_bytes)
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        self.array().map(u64::from_le_bytes)
    }

    pub fn i64(&mut self) -> Result<i64, String> {
        self.array().map(i64::from_le_bytes)
    }

    pub fn f32(&mut self) -> Result<f32, String> {
        self.array().map(f32::from_le_bytes)
    }

    /// A `bool` serialized as a 32-bit 0 or 1; anything else means the data isn't what it's taken
    /// to be.
    pub fn bool32(&mut self) -> Result<bool, String> {
        match self.u32()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(format!("expected a bool, found {other} at {}", self.at - 4)),
        }
    }

    /// A count of things at least `each` bytes long that must fit in what's left.
    pub fn count(&mut self, each: usize) -> Result<usize, String> {
        let count = self.i32()?;
        let left = self.data.len() - self.at;
        usize::try_from(count)
            .ok()
            .filter(|&count| count.saturating_mul(each.max(1)) <= left)
            .ok_or_else(|| format!("unexpected count {count} at {}", self.at - 4))
    }

    /// An `FString`: a length counting the terminating zero, negative for UTF-16.
    pub fn string(&mut self) -> Result<String, String> {
        let length = self.i32()?;
        let text = match length {
            0 => return Ok(String::new()),
            1.. => {
                let bytes = self.take(length as usize)?;
                bytes
                    .iter()
                    .map(|&byte| char::from(byte))
                    .collect::<String>()
            }
            _ => {
                let units = (length.unsigned_abs() as usize)
                    .checked_mul(2)
                    .ok_or("unexpected string length")?;
                let bytes = self.take(units)?;
                let units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&unit| u16::from_le_bytes(unit))
                    .collect();
                String::from_utf16_lossy(&units)
            }
        };
        Ok(text.trim_end_matches('\0').to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_checked() {
        let data = [1, 0, 0, 0, 2, 0];
        let mut bytes = Bytes::new(&data);
        assert_eq!(bytes.u32(), Ok(1));
        assert!(bytes.u32().is_err());
        assert_eq!(bytes.u16(), Ok(2));
    }

    #[test]
    fn strings() {
        let mut data = vec![4, 0, 0, 0, b'a', b'b', b'c', 0];
        data.extend((-3i32).to_le_bytes());
        data.extend([b'h', 0, b'i', 0, 0, 0]);
        let mut bytes = Bytes::new(&data);
        assert_eq!(bytes.string().as_deref(), Ok("abc"));
        assert_eq!(bytes.string().as_deref(), Ok("hi"));
    }

    #[test]
    fn counts_must_fit() {
        let data = 1000i32.to_le_bytes();
        assert!(Bytes::new(&data).count(1).is_err());
        let data = [2, 0, 0, 0, 0, 0];
        assert_eq!(Bytes::new(&data).count(1), Ok(2));
    }
}
