use crate::models::{AppError, AppResult, ErrorCode};
use serde_json::Value;

pub const MAX_ARRAY_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_OBJECTS: usize = 24_000;

#[derive(Default)]
pub struct JsonArrayParser {
    pub buffer: Vec<u8>,
    pub depth: usize,
    pub quoted: bool,
    pub escaped: bool,
}

pub fn invalid_dump(detail: impl AsRef<str>) -> AppError {
    AppError::new(ErrorCode::InvalidData, "Registro JSON PipeWire non valido.").detail(detail)
}

impl JsonArrayParser {
    pub fn feed(&mut self, bytes: &[u8]) -> AppResult<Vec<Vec<Value>>> {
        let mut arrays = Vec::new();
        for byte in bytes {
            if self.buffer.is_empty() {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                if *byte != b'[' {
                    return Err(invalid_dump("Atteso un array JSON senza colori."));
                }
            }
            if self.buffer.len() == MAX_ARRAY_BYTES {
                return Err(invalid_dump("Array oltre il limite di 16 MiB."));
            }
            self.buffer.push(*byte);
            if self.quoted {
                if self.escaped {
                    self.escaped = false;
                } else if *byte == b'\\' {
                    self.escaped = true;
                } else if *byte == b'"' {
                    self.quoted = false;
                }
                continue;
            }
            match byte {
                b'"' => self.quoted = true,
                b'[' | b'{' => {
                    self.depth += 1;
                    if self.depth > 128 {
                        return Err(invalid_dump("JSON troppo annidato."));
                    }
                }
                b']' | b'}' => {
                    self.depth = self
                        .depth
                        .checked_sub(1)
                        .ok_or_else(|| invalid_dump("Chiusura JSON inattesa."))?;
                    if self.depth == 0 {
                        let array: Vec<Value> = serde_json::from_slice(&self.buffer)
                            .map_err(|error| invalid_dump(error.to_string()))?;
                        if array.len() > MAX_OBJECTS {
                            return Err(invalid_dump("Troppi oggetti PipeWire."));
                        }
                        arrays.push(array);
                        self.buffer.clear();
                    }
                }
                _ => {}
            }
        }
        Ok(arrays)
    }

    pub fn finish(&self) -> AppResult<()> {
        if self.buffer.is_empty() {
            Ok(())
        } else {
            Err(invalid_dump("Array JSON incompleto."))
        }
    }
}
