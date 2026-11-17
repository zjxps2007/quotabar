// Streaming projection of QuotaBar/AIUsage session records. See NOTICE.
// Unselected strings (prompts, tool arguments and responses) never enter the output.
use crate::files::invalid;
use memchr::{memchr, memchr2, memchr3};
use serde_json::{Map, Value};
use std::io::{self, BufRead};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq)]
enum Context {
    Root,
    Payload,
    Info,
    Tokens,
    Message,
    Usage,
    Scalar,
    Skip,
}

impl Context {
    fn field(self, key: &str) -> Self {
        match (self, key) {
            (Self::Root, "payload") => Self::Payload,
            (Self::Root, "message") => Self::Message,
            (Self::Payload, "info") => Self::Info,
            (Self::Info, "total_token_usage" | "last_token_usage") => Self::Tokens,
            (Self::Message, "usage") => Self::Usage,
            (Self::Root, "timestamp" | "type")
            | (
                Self::Payload,
                "type" | "id" | "forked_from_id" | "forkedFromId" | "timestamp" | "model",
            )
            | (Self::Info, "model" | "model_name")
            | (Self::Message, "id" | "model")
            | (Self::Tokens, "input_tokens" | "cached_input_tokens" | "output_tokens")
            | (
                Self::Usage,
                "input_tokens"
                | "cache_read_input_tokens"
                | "cache_creation_input_tokens"
                | "output_tokens",
            ) => Self::Scalar,
            (Self::Root, "isSidechain")
            | (Self::Payload, "model_name" | "forked_from_timestamp" | "fork_timestamp")
                if cfg!(target_os = "macos") =>
            {
                Self::Scalar
            }
            _ => Self::Skip,
        }
    }
}

pub(super) enum Row {
    Data(Value),
    Invalid,
    End,
}
pub(super) struct Projection<'a, R> {
    input: R,
    cancelled: &'a AtomicBool,
    captured: usize,
}

impl<'a, R: BufRead> Projection<'a, R> {
    pub fn new(input: R, cancelled: &'a AtomicBool) -> Self {
        Self {
            input,
            cancelled,
            captured: 0,
        }
    }
    fn peek(&mut self) -> io::Result<Option<u8>> {
        Ok(self.input.fill_buf()?.first().copied())
    }
    fn take(&mut self) -> io::Result<u8> {
        match self.peek()? {
            Some(b'\n') => Err(invalid("Unexpected end of row")),
            Some(byte) => {
                self.input.consume(1);
                Ok(byte)
            }
            None => Err(io::ErrorKind::UnexpectedEof.into()),
        }
    }
    fn expect(&mut self, byte: u8) -> io::Result<()> {
        if self.take()? == byte {
            Ok(())
        } else {
            Err(invalid("Unexpected JSON delimiter"))
        }
    }
    fn space(&mut self) -> io::Result<()> {
        while matches!(self.peek()?, Some(b' ' | b'\t' | b'\r')) {
            self.input.consume(1);
        }
        Ok(())
    }
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        Ok(())
    }
    fn string(&mut self, limit: usize) -> io::Result<Option<String>> {
        self.expect(b'"')?;
        let mut bytes = (limit != 0).then(|| vec![b'"']);
        loop {
            self.check()?;
            let buffer = self.input.fill_buf()?;
            if buffer.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            let pos = memchr3(b'"', b'\\', b'\n', buffer).unwrap_or(buffer.len());
            let delimiter = buffer.get(pos).copied();
            if cfg!(windows) && buffer[..pos].iter().any(|byte| *byte < 32) {
                return Err(invalid("Control character in JSON string"));
            }
            let used = pos + usize::from(delimiter.is_some_and(|d| d != b'\n'));
            if let Some(value) = &mut bytes {
                if value.len() + used <= limit {
                    value.extend_from_slice(&buffer[..used]);
                } else {
                    bytes = None;
                }
            }
            self.input.consume(used);
            match delimiter {
                Some(b'\n') => return Err(invalid("Line break in JSON string")),
                Some(b'"') => {
                    return bytes
                        .map(|value| serde_json::from_slice(&value).map_err(io::Error::from))
                        .transpose();
                }
                Some(b'\\') => {
                    let escaped = self.take()?;
                    if cfg!(windows) && escaped < 32 {
                        return Err(invalid("Control character in JSON string"));
                    }
                    if let Some(value) = &mut bytes {
                        value.push(escaped);
                    }
                }
                _ => {}
            }
        }
    }

    // macOS skips unselected containers without imposing a depth limit on tool arguments.
    fn skip_container(&mut self) -> io::Result<()> {
        self.take()?;
        let mut depth = 1usize;
        while depth != 0 {
            self.check()?;
            match self.peek()? {
                Some(b'"') => {
                    self.string(0)?;
                }
                Some(b'{' | b'[') => {
                    self.input.consume(1);
                    depth += 1;
                }
                Some(b'}' | b']') => {
                    self.input.consume(1);
                    depth -= 1;
                }
                Some(b'\n') => return Err(invalid("Line break in JSON container")),
                None => return Err(io::ErrorKind::UnexpectedEof.into()),
                Some(_) => {
                    let buf = self.input.fill_buf()?;
                    let end = [
                        memchr3(b'{', b'[', b'"', buf),
                        memchr2(b'}', b']', buf),
                        memchr(b'\n', buf),
                    ]
                    .into_iter()
                    .flatten()
                    .min()
                    .unwrap_or(buf.len());
                    self.input.consume(end);
                }
            }
        }
        Ok(())
    }

    fn value(&mut self, ctx: Context, depth: usize) -> io::Result<Value> {
        self.check()?;
        if cfg!(windows) && depth > 64 {
            return Err(invalid("Excessive JSON nesting"));
        }
        self.space()?;
        match self.peek()? {
            Some(b'"') => {
                let limit = if ctx == Context::Scalar {
                    if cfg!(windows) { 4096 } else { 16 * 1024 }
                } else {
                    0
                };
                let text = self.string(limit)?;
                if limit != 0 && text.is_none() && cfg!(target_os = "macos") {
                    return Err(invalid("Oversized retained field"));
                }
                if let Some(s) = text {
                    self.captured += s.len();
                    if self.captured > 1024 * 1024 {
                        return Err(invalid("Oversized usage record"));
                    }
                    Ok(Value::String(s))
                } else {
                    Ok(Value::Null)
                }
            }
            Some(b'{') | Some(b'[') => {
                if cfg!(target_os = "macos")
                    && (matches!(ctx, Context::Skip | Context::Scalar)
                        || self.peek()? == Some(b'['))
                {
                    self.skip_container()?;
                    return Ok(Value::Null);
                }
                let object = self.take()? == b'{';
                let end = if object { b'}' } else { b']' };
                let keep = object && ctx != Context::Scalar && ctx != Context::Skip;
                let mut fields = Map::new();
                self.space()?;
                if self.peek()? == Some(end) {
                    self.input.consume(1);
                } else {
                    loop {
                        self.space()?;
                        let key = if object {
                            let key = self.string(if keep {
                                if cfg!(windows) { 4096 } else { 1024 }
                            } else {
                                0
                            })?;
                            self.space()?;
                            self.expect(b':')?;
                            key
                        } else {
                            None
                        };
                        let child = key.as_deref().map_or(Context::Skip, |s| ctx.field(s));
                        let value = self.value(child, depth + 1)?;
                        if child != Context::Skip
                            && let Some(name) = key
                        {
                            fields.insert(name, value);
                        }
                        self.space()?;
                        let delimiter = self.take()?;
                        if delimiter == end {
                            break;
                        }
                        if delimiter != b',' {
                            return Err(invalid("Invalid container delimiter"));
                        }
                    }
                }
                Ok(if keep {
                    Value::Object(fields)
                } else {
                    Value::Null
                })
            }
            None => Err(io::ErrorKind::UnexpectedEof.into()),
            _ => {
                let mut scalar = Vec::new();
                let mut consumed = false;
                while let Some(b) = self.peek()? {
                    if matches!(b, b',' | b']' | b'}' | b' ' | b'\r' | b'\n' | b'\t') {
                        break;
                    }
                    self.input.consume(1);
                    consumed = true;
                    if ctx != Context::Skip {
                        if scalar.len() >= if cfg!(windows) { 64 } else { 128 } {
                            return Err(invalid("Oversized scalar"));
                        }
                        scalar.push(b);
                    }
                    // Bare tokens can also be arbitrarily long in a damaged input.
                    self.check()?;
                }
                if !consumed {
                    return Err(invalid("Empty JSON scalar"));
                }
                if ctx == Context::Scalar {
                    Ok(serde_json::from_slice(&scalar)?)
                } else {
                    Ok(Value::Null)
                }
            }
        }
    }

    pub fn row(&mut self) -> io::Result<Row> {
        loop {
            self.check()?;
            self.space()?;
            match self.peek()? {
                Some(b'\n') => self.input.consume(1),
                None => return Ok(Row::End),
                _ => break,
            }
        }
        self.captured = 0;
        let parse = (|| {
            let value = self.value(Context::Root, 0)?;
            self.space()?;
            match self.peek()? {
                Some(b'\n') => self.input.consume(1),
                None => {}
                _ => return Err(invalid("Unexpected data after JSON row")),
            }
            Ok(value)
        })();
        match parse {
            Ok(value) => Ok(Row::Data(value)),
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(Row::End),
            Err(err) if err.kind() == io::ErrorKind::InvalidData => loop {
                self.check()?;
                let buffer = self.input.fill_buf()?;
                if buffer.is_empty() {
                    return Ok(if cfg!(windows) {
                        Row::Invalid
                    } else {
                        Row::End
                    });
                }
                if let Some(end) = memchr(b'\n', buffer) {
                    self.input.consume(end + 1);
                    return Ok(Row::Invalid);
                }
                let len = buffer.len();
                self.input.consume(len);
            },
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn skips_escaped_bodies_across_buffers_and_recovers_after_malformed_lines() {
        let data = br#"{"ty\u0070e":"assistant","message":{"content":[{"args":"private\"text\\tail"}],"id":"kept","usage":{"input_tokens":3}}}
{broken}
{"timestamp":"final"}
{"unfinished":"#;
        for size in 1..=12 {
            let flag = AtomicBool::new(false);
            let mut input =
                Projection::new(BufReader::with_capacity(size, Cursor::new(data)), &flag);
            let Row::Data(row) = input.row().unwrap() else {
                panic!("missing first row")
            };
            assert_eq!(row["type"], "assistant");
            assert_eq!(row["message"]["id"], "kept");
            assert_eq!(row["message"]["usage"]["input_tokens"], 3);
            assert!(!row.to_string().contains("private"));
            assert!(matches!(input.row().unwrap(), Row::Invalid));
            let Row::Data(row) = input.row().unwrap() else {
                panic!("missing last record")
            };
            assert_eq!(row["timestamp"], "final");
            assert!(matches!(input.row().unwrap(), Row::End));
        }
    }
}
