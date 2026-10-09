//! A lossless iCalendar document layer.
//!
//! Read-modify-write must keep every property it does not touch byte for byte
//! (Apple's `X-APPLE-STRUCTURED-LOCATION` with quoted base64 parameters,
//! `X-WR-ALARMUID`, `ACKNOWLEDGED`, attendee parameters, ...). Parsing keeps
//! each content line's original text, folding and line ending; an untouched
//! line is written back verbatim and an edited or new line is encoded fresh,
//! folded at 75 octets on a UTF-8 boundary with CRLF endings.

/// One parameter: `NAME=value[,value...]` with quotes removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub values: Vec<String>,
}

/// One content line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    /// Uppercase name.
    pub name: String,
    pub params: Vec<Param>,
    /// The raw (still escaped) value.
    pub value: String,
    /// The exact original text, folding and line ending included. `None` once
    /// edited or for a new property.
    raw: Option<String>,
}

impl Property {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: value.into(),
            raw: None,
        }
    }

    /// A TEXT property with its value escaped.
    pub fn text(name: &str, value: &str) -> Self {
        Self::new(name, escape_text(value))
    }

    pub fn with_param(mut self, name: &str, value: &str) -> Self {
        self.set_param(name, value);
        self
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .and_then(|p| p.values.first())
            .map(String::as_str)
    }

    pub fn set_param(&mut self, name: &str, value: &str) {
        self.raw = None;
        let upper = name.to_ascii_uppercase();
        match self.params.iter_mut().find(|p| p.name == upper) {
            Some(param) => param.values = vec![value.to_owned()],
            None => self.params.push(Param {
                name: upper,
                values: vec![value.to_owned()],
            }),
        }
    }

    /// The unescaped TEXT value.
    pub fn text_value(&self) -> String {
        unescape_text(&self.value)
    }

    fn encode(&self) -> String {
        let mut line = self.name.clone();
        for param in &self.params {
            line.push(';');
            line.push_str(&param.name);
            line.push('=');
            let encoded: Vec<String> = param.values.iter().map(|v| quote_param(v)).collect();
            line.push_str(&encoded.join(","));
        }
        line.push(':');
        line.push_str(&self.value);
        fold(&line)
    }
}

/// A component's ordered content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Prop(Property),
    Child(Component),
}

/// `BEGIN:<name>` ... `END:<name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    /// Uppercase name.
    pub name: String,
    pub items: Vec<Item>,
    begin_raw: Option<String>,
    end_raw: Option<String>,
}

impl Component {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            items: Vec::new(),
            begin_raw: None,
            end_raw: None,
        }
    }

    pub fn props(&self) -> impl Iterator<Item = &Property> {
        self.items.iter().filter_map(|item| match item {
            Item::Prop(p) => Some(p),
            Item::Child(_) => None,
        })
    }

    pub fn children(&self) -> impl Iterator<Item = &Component> {
        self.items.iter().filter_map(|item| match item {
            Item::Child(c) => Some(c),
            Item::Prop(_) => None,
        })
    }

    pub fn children_mut(&mut self) -> impl Iterator<Item = &mut Component> {
        self.items.iter_mut().filter_map(|item| match item {
            Item::Child(c) => Some(c),
            Item::Prop(_) => None,
        })
    }

    pub fn prop(&self, name: &str) -> Option<&Property> {
        self.props().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn props_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.props()
            .filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    /// The unescaped TEXT value of the first property named `name`.
    pub fn text(&self, name: &str) -> Option<String> {
        self.prop(name).map(Property::text_value)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.prop(name).map(|p| p.value.as_str())
    }

    /// Replaces every property named like `property` with it, at the first
    /// one's position (appended before the first child when absent).
    pub fn set(&mut self, property: Property) {
        let name = property.name.clone();
        let first = self
            .items
            .iter()
            .position(|item| matches!(item, Item::Prop(p) if p.name == name));
        self.remove(&name);
        match first {
            Some(index) => self
                .items
                .insert(index.min(self.items.len()), Item::Prop(property)),
            None => self.insert_prop(property),
        }
    }

    /// Adds a property after the existing properties (before children).
    pub fn insert_prop(&mut self, property: Property) {
        let index = self
            .items
            .iter()
            .position(|item| matches!(item, Item::Child(_)))
            .unwrap_or(self.items.len());
        self.items.insert(index, Item::Prop(property));
    }

    /// Removes every property named `name`; returns how many went.
    pub fn remove(&mut self, name: &str) -> usize {
        let before = self.items.len();
        self.items
            .retain(|item| !matches!(item, Item::Prop(p) if p.name.eq_ignore_ascii_case(name)));
        before - self.items.len()
    }

    pub fn retain_props(&mut self, mut keep: impl FnMut(&Property) -> bool) {
        self.items.retain(|item| match item {
            Item::Prop(p) => keep(p),
            Item::Child(_) => true,
        });
    }

    pub fn retain_children(&mut self, mut keep: impl FnMut(&Component) -> bool) {
        self.items.retain(|item| match item {
            Item::Child(c) => keep(c),
            Item::Prop(_) => true,
        });
    }

    pub fn push_child(&mut self, child: Component) {
        self.items.push(Item::Child(child));
    }

    fn serialize_into(&self, out: &mut String) {
        match &self.begin_raw {
            Some(raw) => out.push_str(raw),
            None => {
                out.push_str("BEGIN:");
                out.push_str(&self.name);
                out.push_str("\r\n");
            }
        }
        for item in &self.items {
            match item {
                Item::Prop(p) => match &p.raw {
                    Some(raw) => out.push_str(raw),
                    None => out.push_str(&p.encode()),
                },
                Item::Child(c) => c.serialize_into(out),
            }
        }
        match &self.end_raw {
            Some(raw) => out.push_str(raw),
            None => {
                out.push_str("END:");
                out.push_str(&self.name);
                out.push_str("\r\n");
            }
        }
    }
}

/// A parsed iCalendar object (`VCALENDAR`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcsDoc {
    pub root: Component,
    /// Text after the final `END:VCALENDAR` (kept for byte identity).
    trailer: String,
}

/// Why a body is not an iCalendar object.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid iCalendar: {0}")]
pub struct IcsError(pub String);

impl IcsDoc {
    pub fn new(root: Component) -> Self {
        Self {
            root,
            trailer: String::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Self, IcsError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let lines = split_content_lines(text);
        let mut stack: Vec<Component> = Vec::new();
        let mut root: Option<Component> = None;
        let mut trailer = String::new();
        for (raw, logical) in lines {
            if root.is_some() {
                trailer.push_str(raw);
                continue;
            }
            if logical.trim().is_empty() {
                // Blank lines are tolerated; keep them attached for identity.
                match stack.last_mut() {
                    Some(top) => top.items.push(Item::Prop(Property {
                        name: String::new(),
                        params: Vec::new(),
                        value: String::new(),
                        raw: Some(raw.to_owned()),
                    })),
                    None => return Err(IcsError("text before BEGIN:VCALENDAR".to_owned())),
                }
                continue;
            }
            let property = parse_line(&logical)
                .ok_or_else(|| IcsError(format!("malformed line: {}", truncate(&logical))))?;
            match property.name.as_str() {
                "BEGIN" => {
                    let mut component = Component::new(property.value.trim());
                    component.begin_raw = Some(raw.to_owned());
                    stack.push(component);
                }
                "END" => {
                    let name = property.value.trim().to_ascii_uppercase();
                    let Some(mut component) = stack.pop() else {
                        return Err(IcsError(format!("END:{name} without BEGIN")));
                    };
                    if component.name != name {
                        return Err(IcsError(format!(
                            "END:{name} closes BEGIN:{}",
                            component.name
                        )));
                    }
                    component.end_raw = Some(raw.to_owned());
                    match stack.last_mut() {
                        Some(parent) => parent.items.push(Item::Child(component)),
                        None => root = Some(component),
                    }
                }
                _ => {
                    let Some(top) = stack.last_mut() else {
                        return Err(IcsError("property outside a component".to_owned()));
                    };
                    top.items.push(Item::Prop(Property {
                        raw: Some(raw.to_owned()),
                        ..property
                    }));
                }
            }
        }
        if !stack.is_empty() {
            return Err(IcsError("unterminated component".to_owned()));
        }
        let root = root.ok_or_else(|| IcsError("no VCALENDAR".to_owned()))?;
        if root.name != "VCALENDAR" {
            return Err(IcsError(format!("root is {}", root.name)));
        }
        Ok(Self { root, trailer })
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        self.root.serialize_into(&mut out);
        out.push_str(&self.trailer);
        out
    }

    pub fn events(&self) -> impl Iterator<Item = &Component> {
        self.root.children().filter(|c| c.name == "VEVENT")
    }

    pub fn events_mut(&mut self) -> impl Iterator<Item = &mut Component> {
        self.root.children_mut().filter(|c| c.name == "VEVENT")
    }

    pub fn timezones(&self) -> impl Iterator<Item = &Component> {
        self.root.children().filter(|c| c.name == "VTIMEZONE")
    }

    pub fn timezone(&self, tzid: &str) -> Option<&Component> {
        self.timezones().find(|c| c.value("TZID") == Some(tzid))
    }
}

fn truncate(text: &str) -> String {
    text.chars().take(80).collect()
}

/// Physical lines grouped into logical content lines: each item is the exact
/// raw text (with folds and line endings) and the unfolded line without its
/// terminator.
fn split_content_lines(text: &str) -> Vec<(&str, String)> {
    let mut out: Vec<(&str, String)> = Vec::new();
    let bytes = text.as_bytes();
    let mut group: Option<(usize, String)> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let line_start = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let content_end = if i > line_start && bytes[i - 1] == b'\r' {
            i - 1
        } else {
            i
        };
        if i < bytes.len() {
            i += 1;
        }
        let physical = &text[line_start..content_end];
        match group.as_mut() {
            Some((_, logical)) if physical.starts_with([' ', '\t']) => {
                logical.push_str(&physical[1..]);
            }
            _ => {
                if let Some((start, logical)) = group.take() {
                    out.push((&text[start..line_start], logical));
                }
                group = Some((line_start, physical.to_owned()));
            }
        }
    }
    if let Some((start, logical)) = group {
        out.push((&text[start..], logical));
    }
    out
}

/// `NAME *(;param) : value`, honouring quoted parameter values.
fn parse_line(line: &str) -> Option<Property> {
    let mut name_end = None;
    for (i, c) in line.char_indices() {
        if c == ';' || c == ':' {
            name_end = Some(i);
            break;
        }
    }
    let name_end = name_end?;
    let name = line[..name_end].trim().to_ascii_uppercase();
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return None;
    }
    let mut params = Vec::new();
    let mut rest = &line[name_end..];
    while let Some(after) = rest.strip_prefix(';') {
        let eq = after.find('=')?;
        let pname = after[..eq].trim().to_ascii_uppercase();
        let mut values = Vec::new();
        let mut tail = &after[eq + 1..];
        loop {
            if let Some(quoted) = tail.strip_prefix('"') {
                let end = quoted.find('"')?;
                values.push(quoted[..end].to_owned());
                tail = &quoted[end + 1..];
            } else {
                let end = tail.find([',', ';', ':']).unwrap_or(tail.len());
                values.push(tail[..end].to_owned());
                tail = &tail[end..];
            }
            match tail.strip_prefix(',') {
                Some(next) => tail = next,
                None => break,
            }
        }
        params.push(Param {
            name: pname,
            values,
        });
        rest = tail;
    }
    let value = rest.strip_prefix(':')?;
    Some(Property {
        name,
        params,
        value: value.to_owned(),
        raw: None,
    })
}

fn quote_param(value: &str) -> String {
    let clean: String = value.chars().filter(|c| *c != '"').collect();
    if clean.contains([':', ';', ',']) {
        format!("\"{clean}\"")
    } else {
        clean
    }
}

/// Folds a content line at 75 octets on UTF-8 boundaries, CRLF terminated.
pub fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut width = 0usize;
    for c in line.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
    out
}

/// Escapes a TEXT value (`\`, `;`, `,`, newline).
pub fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

/// Reverses [`escape_text`] (`\n` and `\N` are newlines; an unknown escape keeps
/// the character).
pub fn unescape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Apple Inc.//macOS 15//EN\r\nBEGIN:VEVENT\r\nUID:ABC\r\nX-APPLE-STRUCTURED-LOCATION;VALUE=URI;X-ADDRESS=\"1 Main St, Vancouver\";X-TITLE=\"Clinic; 2nd floor\":geo:49.2,-123.1\r\nSUMMARY:Vet\\, follow-up\r\nDESCRIPTION:A very long description that certainly goes on past seventy-five oc\r\n tets so it must be folded\r\nBEGIN:VALARM\r\nX-WR-ALARMUID:1\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn round_trips_byte_for_byte() {
        let doc = IcsDoc::parse(APPLE).unwrap();
        assert_eq!(doc.serialize(), APPLE);
        let lf = APPLE.replace("\r\n", "\n");
        assert_eq!(IcsDoc::parse(&lf).unwrap().serialize(), lf);
    }

    #[test]
    fn reads_quoted_params_and_unfolds() {
        let doc = IcsDoc::parse(APPLE).unwrap();
        let event = doc.events().next().unwrap();
        let loc = event.prop("X-APPLE-STRUCTURED-LOCATION").unwrap();
        assert_eq!(loc.param("X-TITLE"), Some("Clinic; 2nd floor"));
        assert_eq!(loc.value, "geo:49.2,-123.1");
        assert_eq!(event.text("SUMMARY").as_deref(), Some("Vet, follow-up"));
        assert!(
            event
                .text("DESCRIPTION")
                .unwrap()
                .ends_with("seventy-five octets so it must be folded")
        );
    }

    #[test]
    fn edits_only_touched_lines() {
        let mut doc = IcsDoc::parse(APPLE).unwrap();
        let event = doc.events_mut().next().unwrap();
        event.set(Property::text("SUMMARY", "New; title"));
        let out = doc.serialize();
        assert!(out.contains("SUMMARY:New\\; title\r\n"));
        assert!(out.contains("X-TITLE=\"Clinic; 2nd floor\""));
        assert!(out.contains("oc\r\n tets"));
    }

    #[test]
    fn folds_on_utf8_boundaries() {
        let line = format!("SUMMARY:{}", "é".repeat(60));
        let folded = fold(&line);
        for physical in folded.split("\r\n") {
            assert!(physical.len() <= 75, "{physical}");
        }
        let doc = IcsDoc::parse(&format!(
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n{folded}END:VEVENT\r\nEND:VCALENDAR\r\n"
        ))
        .unwrap();
        assert_eq!(
            doc.events().next().unwrap().text("SUMMARY").unwrap(),
            "é".repeat(60)
        );
    }

    #[test]
    fn rejects_unbalanced_components() {
        assert!(IcsDoc::parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VCALENDAR\r\n").is_err());
        assert!(IcsDoc::parse("hello").is_err());
    }

    #[test]
    fn text_escapes_round_trip() {
        let text = "a\\b;c,d\ne";
        assert_eq!(unescape_text(&escape_text(text)), text);
    }
}
