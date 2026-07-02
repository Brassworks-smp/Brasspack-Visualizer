
use serde_json::{Map, Number, Value};

pub fn parse(input: &str) -> Option<Value> {
    let bytes = input.as_bytes();
    let mut p = Parser { b: bytes, i: 0 };
    p.skip_ws();
    let v = p.value()?;
    Some(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' | b'\'' => Some(Value::String(self.quoted())),
            _ => Some(self.bare()),
        }
    }

    fn object(&mut self) -> Option<Value> {
        self.i += 1;
        let mut map = Map::new();
        loop {
            self.skip_ws();
            match self.peek()? {
                b'}' => {
                    self.i += 1;
                    break;
                }
                b',' => {
                    self.i += 1;
                    continue;
                }
                _ => {}
            }
            let key = match self.peek()? {
                b'"' | b'\'' => self.quoted(),
                _ => self.bare_token(),
            };
            self.skip_ws();
            if self.peek() == Some(b':') {
                self.i += 1;
            } else {
                return None;
            }
            let val = self.value()?;
            map.insert(key, val);
        }
        Some(Value::Object(map))
    }

    fn array(&mut self) -> Option<Value> {
        self.i += 1;
        let save = self.i;
        self.skip_ws();
        if let (Some(t), Some(b';')) = (self.peek(), self.b.get(self.i + 1).copied()) {
            if matches!(t, b'B' | b'I' | b'L' | b'b' | b'i' | b'l') {
                self.i += 2;
            } else {
                self.i = save;
            }
        } else {
            self.i = save;
        }
        let mut arr = Vec::new();
        loop {
            self.skip_ws();
            match self.peek()? {
                b']' => {
                    self.i += 1;
                    break;
                }
                b',' => {
                    self.i += 1;
                    continue;
                }
                _ => {
                    arr.push(self.value()?);
                }
            }
        }
        Some(Value::Array(arr))
    }

    fn quoted(&mut self) -> String {
        let quote = self.b[self.i];
        self.i += 1;
        let mut s = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'\\' => {
                    if let Some(next) = self.peek() {
                        self.i += 1;
                        match next {
                            b'n' => s.push('\n'),
                            b't' => s.push('\t'),
                            other => s.push(other as char),
                        }
                    }
                }
                c if c == quote => break,
                _ => s.push(c as char),
            }
        }
        s
    }

    fn bare(&mut self) -> Value {
        let tok = self.bare_token();
        interpret(&tok)
    }

    fn bare_token(&mut self) -> String {
        let start = self.i;
        while let Some(c) = self.peek() {
            if matches!(c, b',' | b'}' | b']' | b':' | b'{' | b'[') || c.is_ascii_whitespace() {
                break;
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.b[start..self.i]).into_owned()
    }
}

fn interpret(tok: &str) -> Value {
    match tok {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        "" => return Value::String(String::new()),
        _ => {}
    }
    let core = if tok.len() > 1 {
        let last = tok.as_bytes()[tok.len() - 1];
        if matches!(last, b'b' | b's' | b'l' | b'f' | b'd' | b'B' | b'S' | b'L' | b'F' | b'D') {
            &tok[..tok.len() - 1]
        } else {
            tok
        }
    } else {
        tok
    };
    if let Ok(i) = core.parse::<i64>() {
        return Value::Number(i.into());
    }
    if let Ok(f) = core.parse::<f64>() {
        if let Some(n) = Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    Value::String(tok.to_string())
}
