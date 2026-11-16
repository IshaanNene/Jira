//! The small part of CBOR (RFC 8949) that WebAuthn uses: definite-length integers, byte and
//! text strings, arrays, maps, and simple values. Enough to read an attestation object and a
//! COSE public key, and nothing more. Input comes from browsers and authenticators, so every
//! length is checked against what is left and nesting is limited.

use std::collections::BTreeMap;

const MAX_DEPTH: usize = 8;
const MAX_ITEMS: u64 = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    /// Unsigned and negative integers, which COSE keys use as map keys and algorithm ids.
    Int(i128),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    Map(BTreeMap<Value, Value>),
    Bool(bool),
    Null,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed CBOR: {}", self.0)
    }
}

impl std::error::Error for Error {}

impl Value {
    pub fn as_map(&self) -> Option<&BTreeMap<Value, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i128> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
}

/// Decode one item from the front of `input`; returns it and how many bytes it took.
/// Trailing bytes are the caller's business (authenticator data carries extensions after
/// the public key).
pub fn decode_prefix(input: &[u8]) -> Result<(Value, usize), Error> {
    let mut reader = Reader { input, pos: 0 };
    let value = reader.item(0)?;
    Ok((value, reader.pos))
}

/// Decode exactly one item; anything after it is an error.
pub fn decode(input: &[u8]) -> Result<Value, Error> {
    let (value, used) = decode_prefix(input)?;
    if used != input.len() {
        return Err(Error("trailing bytes"));
    }
    Ok(value)
}

struct Reader<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self.pos.checked_add(n).filter(|&end| end <= self.input.len()).ok_or(Error("truncated"))?;
        let bytes = &self.input[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn argument(&mut self, info: u8) -> Result<u64, Error> {
        Ok(match info {
            0..=23 => u64::from(info),
            24 => u64::from(self.take(1)?[0]),
            25 => u64::from(u16::from_be_bytes(self.take(2)?.try_into().map_err(|_| Error("truncated"))?)),
            26 => u64::from(u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| Error("truncated"))?)),
            27 => u64::from_be_bytes(self.take(8)?.try_into().map_err(|_| Error("truncated"))?),
            _ => return Err(Error("indefinite or reserved length")),
        })
    }

    fn length(&mut self, info: u8) -> Result<usize, Error> {
        let n = self.argument(info)?;
        // A length can never exceed what is left; checking here avoids huge allocations.
        usize::try_from(n).ok().filter(|&n| n <= self.input.len() - self.pos).ok_or(Error("length past end"))
    }

    fn item(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(Error("nested too deeply"));
        }
        let head = self.take(1)?[0];
        let (major, info) = (head >> 5, head & 0x1f);
        match major {
            0 => Ok(Value::Int(i128::from(self.argument(info)?))),
            1 => Ok(Value::Int(-1 - i128::from(self.argument(info)?))),
            2 => {
                let n = self.length(info)?;
                Ok(Value::Bytes(self.take(n)?.to_vec()))
            }
            3 => {
                let n = self.length(info)?;
                let text = std::str::from_utf8(self.take(n)?).map_err(|_| Error("invalid UTF-8"))?;
                Ok(Value::Text(text.to_owned()))
            }
            4 => {
                let n = self.argument(info)?;
                if n > MAX_ITEMS {
                    return Err(Error("too many items"));
                }
                (0..n).map(|_| self.item(depth + 1)).collect::<Result<_, _>>().map(Value::Array)
            }
            5 => {
                let n = self.argument(info)?;
                if n > MAX_ITEMS {
                    return Err(Error("too many items"));
                }
                let mut map = BTreeMap::new();
                for _ in 0..n {
                    let key = self.item(depth + 1)?;
                    let value = self.item(depth + 1)?;
                    if map.insert(key, value).is_some() {
                        return Err(Error("duplicate map key"));
                    }
                }
                Ok(Value::Map(map))
            }
            7 => match info {
                20 => Ok(Value::Bool(false)),
                21 => Ok(Value::Bool(true)),
                22 => Ok(Value::Null),
                _ => Err(Error("unsupported simple value")),
            },
            _ => Err(Error("tags are not supported")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_rfc_examples() {
        assert_eq!(decode(&[0x00]).unwrap(), Value::Int(0));
        assert_eq!(decode(&[0x18, 0x64]).unwrap(), Value::Int(100));
        assert_eq!(decode(&[0x39, 0x01, 0x00]).unwrap(), Value::Int(-257));
        assert_eq!(decode(&[0x20]).unwrap(), Value::Int(-1));
        assert_eq!(decode(&[0x44, 1, 2, 3, 4]).unwrap(), Value::Bytes(vec![1, 2, 3, 4]));
        assert_eq!(decode(&[0x64, b'I', b'E', b'T', b'F']).unwrap(), Value::Text("IETF".into()));
        assert_eq!(decode(&[0x83, 1, 2, 3]).unwrap(), Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
        let map = decode(&[0xa2, 0x01, 0x02, 0x61, b'a', 0xf5]).unwrap();
        assert_eq!(map.as_map().unwrap()[&Value::Int(1)], Value::Int(2));
        assert_eq!(map.as_map().unwrap()[&Value::Text("a".into())], Value::Bool(true));
    }

    #[test]
    fn refuses_what_could_hurt() {
        // Lengths past the end, and a 2^64 length that must not be allocated.
        assert_eq!(decode(&[0x45, 1, 2]), Err(Error("length past end")));
        assert!(decode(&[0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(decode(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert_eq!(decode(&[0x5f]), Err(Error("indefinite or reserved length")));
        assert_eq!(decode(&[0xc0, 0x00]), Err(Error("tags are not supported")));
        assert_eq!(decode(&[0xa2, 0x01, 0x00, 0x01, 0x00]), Err(Error("duplicate map key")));
        assert_eq!(decode(&[0x62, 0xff, 0xfe]), Err(Error("invalid UTF-8")));
        assert_eq!(decode(&[0x00, 0x00]), Err(Error("trailing bytes")));
        assert_eq!(decode(&[]), Err(Error("truncated")));
        let deep = [vec![0x81; 20], vec![0x00]].concat();
        assert_eq!(decode(&deep), Err(Error("nested too deeply")));
        assert_eq!(decode_prefix(&[0x01, 0xff]).unwrap(), (Value::Int(1), 1));
    }
}
