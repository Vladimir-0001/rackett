//! Bounded structural validation without re-encoding (which could change info hashes).
pub fn validate(bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.first() != Some(&b'd') {
        return Err("Torrent must be a dictionary");
    }
    let mut parser = Parser {
        bytes,
        pos: 1,
        values: 0,
    };
    let mut info = false;
    while parser.peek()? != b'e' {
        let key = parser.string()?;
        let is_info = key == b"info";
        if is_info && parser.peek()? != b'd' {
            return Err("Torrent info must be a dictionary");
        }
        if is_info && info {
            return Err("Duplicate info dictionary");
        }
        parser.value(1)?;
        info |= is_info;
    }
    parser.pos += 1;
    if !info || parser.pos != bytes.len() {
        return Err("Missing info dictionary or trailing data");
    }
    Ok(())
}
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    values: usize,
}
impl<'a> Parser<'a> {
    fn peek(&self) -> Result<u8, &'static str> {
        self.bytes.get(self.pos).copied().ok_or("Truncated torrent")
    }
    fn string(&mut self) -> Result<&'a [u8], &'static str> {
        let start = self.pos;
        while self.peek()?.is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start || self.peek()? != b':' {
            return Err("Invalid byte string");
        }
        let digits = &self.bytes[start..self.pos];
        if digits.len() > 1 && digits[0] == b'0' {
            return Err("Invalid string length");
        }
        let length = digits
            .iter()
            .try_fold(0usize, |n, b| {
                n.checked_mul(10)
                    .and_then(|n| n.checked_add((b - b'0') as usize))
            })
            .ok_or("String length overflow")?;
        self.pos += 1;
        let end = self
            .pos
            .checked_add(length)
            .filter(|n| *n <= self.bytes.len())
            .ok_or("Truncated string")?;
        let value = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(value)
    }
    fn value(&mut self, depth: u32) -> Result<(), &'static str> {
        self.values += 1;
        if depth > 64 || self.values > 100_000 {
            return Err("Torrent complexity budget exceeded");
        }
        match self.peek()? {
            b'0'..=b'9' => {
                self.string()?;
            }
            b'i' => {
                self.pos += 1;
                let start = self.pos;
                if self.peek()? == b'-' {
                    self.pos += 1;
                }
                let digits = self.pos;
                while self.peek()?.is_ascii_digit() {
                    self.pos += 1;
                }
                if self.pos == digits || self.peek()? != b'e' {
                    return Err("Invalid integer");
                }
                let value = &self.bytes[digits..self.pos];
                if value.len() > 1 && value[0] == b'0' || (digits > start && value == b"0") {
                    return Err("Noncanonical integer");
                }
                self.pos += 1;
            }
            b'l' | b'd' => {
                let dictionary = self.peek()? == b'd';
                self.pos += 1;
                while self.peek()? != b'e' {
                    if dictionary {
                        self.string()?;
                    }
                    self.value(depth + 1)?;
                }
                self.pos += 1;
            }
            _ => return Err("Invalid bencode value"),
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::validate;
    #[test]
    fn validates_without_rewriting_unsorted_keys() {
        assert!(validate(b"d4:infod4:name6:ubuntu6:lengthi42eee").is_ok());
    }
    #[test]
    fn rejects_html_empty_truncated_and_trailing() {
        for data in [
            b"".as_slice(),
            b"<html>error</html>",
            b"de",
            b"d4:infodeeextra",
            b"d4:infod4:name9:shortee",
            b"d4:infoi3ee",
            b"d4:infode4:infodee",
        ] {
            assert!(validate(data).is_err(), "{data:?}");
        }
    }
    #[test]
    fn bounds_depth_and_integer_syntax() {
        let mut data = b"d4:info".to_vec();
        data.extend(vec![b'd'; 100]);
        assert!(validate(&data).is_err());
        assert!(validate(b"d4:infod1:ai-0eee").is_err());
    }
}
