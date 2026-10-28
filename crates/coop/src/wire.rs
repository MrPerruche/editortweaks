//! Little-endian encoding for messages. Every length and count read from the wire is checked
//! against a cap before anything is allocated, so a broken or hostile peer can't make us allocate
//! gigabytes or panic.

use std::fmt;

/// Why bytes from a peer couldn't be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The bytes ended in the middle of a value.
    Truncated,
    /// A length or count was over its cap.
    TooLong {
        what: &'static str,
        len: usize,
        max: usize,
    },
    /// An enum tag we don't know.
    BadTag { what: &'static str, tag: u8 },
    /// A string wasn't UTF-8.
    BadUtf8,
    /// Bytes were left over after the message.
    TrailingBytes(usize),
    /// Not one of our messages (wrong magic).
    NotOurs,
    /// One of our messages, but from another protocol version.
    WrongProtocol(u16),
    /// Compressed data that didn't decompress.
    BadCompression,
    /// A value no honest peer sends (e.g. a brick moved to infinity).
    BadValue(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "message ends early"),
            Self::TooLong { what, len, max } => write!(f, "{what}: {len} is over the cap of {max}"),
            Self::BadTag { what, tag } => write!(f, "{what}: unknown tag {tag}"),
            Self::BadUtf8 => write!(f, "string isn't UTF-8"),
            Self::TrailingBytes(n) => write!(f, "{n} bytes left over"),
            Self::NotOurs => write!(f, "not a coop message"),
            Self::WrongProtocol(v) => write!(f, "protocol {v}"),
            Self::BadCompression => write!(f, "compressed data is broken"),
            Self::BadValue(what) => write!(f, "{what}: impossible value"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub type Result<T> = std::result::Result<T, DecodeError>;

/// Builds a message's bytes.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// As its bits, so it arrives exactly as sent.
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }

    /// A count of items to follow. Callers keep counts under the matching read cap.
    pub fn count(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("count fits in u32"));
    }

    /// Length-prefixed bytes.
    pub fn bytes(&mut self, v: &[u8]) {
        self.count(v.len());
        self.buf.extend_from_slice(v);
    }

    /// Bytes without a length (the rest of the message).
    pub fn raw(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

/// Reads a message's bytes in the order a [`Writer`] wrote them.
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.data.len() {
            return Err(DecodeError::Truncated);
        }
        let (head, rest) = self.data.split_at(n);
        self.data = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.take(N)?;
        Ok(bytes.try_into().expect("take returned N bytes"))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(DecodeError::BadTag { what: "bool", tag }),
        }
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// A count, refused when over `max`. Also refused when even one byte per item couldn't fit in
    /// what's left, so a lying count can't make the caller reserve memory.
    pub fn count(&mut self, what: &'static str, max: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(DecodeError::TooLong { what, len: n, max });
        }
        if n > self.data.len() {
            return Err(DecodeError::Truncated);
        }
        Ok(n)
    }

    pub fn bytes(&mut self, what: &'static str, max: usize) -> Result<&'a [u8]> {
        let n = self.count(what, max)?;
        self.take(n)
    }

    pub fn str(&mut self, what: &'static str, max: usize) -> Result<String> {
        let bytes = self.bytes(what, max)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }

    /// Everything not read yet.
    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.data)
    }

    /// Fails if bytes are left over.
    pub fn finish(self) -> Result<()> {
        match self.data.len() {
            0 => Ok(()),
            n => Err(DecodeError::TrailingBytes(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut w = Writer::new();
        w.u8(7);
        w.bool(true);
        w.u16(0xBEEF);
        w.u32(123_456);
        w.u64(u64::MAX - 1);
        w.f32(-0.1);
        w.str("héllo");
        w.bytes(&[1, 2, 3]);
        let bytes = w.finish();

        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8(), Ok(7));
        assert_eq!(r.bool(), Ok(true));
        assert_eq!(r.u16(), Ok(0xBEEF));
        assert_eq!(r.u32(), Ok(123_456));
        assert_eq!(r.u64(), Ok(u64::MAX - 1));
        assert_eq!(r.f32().map(f32::to_bits), Ok((-0.1f32).to_bits()));
        assert_eq!(r.str("s", 100).as_deref(), Ok("héllo"));
        assert_eq!(r.bytes("b", 100), Ok(&[1u8, 2, 3][..]));
        assert_eq!(r.finish(), Ok(()));
    }

    #[test]
    fn caps_and_truncation() {
        let mut w = Writer::new();
        w.bytes(&[0; 10]);
        let bytes = w.finish();
        assert!(matches!(
            Reader::new(&bytes).bytes("b", 5),
            Err(DecodeError::TooLong {
                len: 10,
                max: 5,
                ..
            })
        ));
        assert_eq!(
            Reader::new(&bytes[..8]).bytes("b", 100),
            Err(DecodeError::Truncated)
        );

        // A huge count with nothing behind it is refused before anyone reserves memory for it.
        let lying = u32::MAX.to_le_bytes();
        assert_eq!(
            Reader::new(&lying).count("c", usize::MAX),
            Err(DecodeError::Truncated)
        );
    }
}
