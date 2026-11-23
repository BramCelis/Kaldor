//! A small element tree from an XML document (quick-xml), for the readers
//! of other libraries' files. Text is joined back from quick-xml's text,
//! CDATA and entity events ("Rock &amp; Roll" arrives in three parts).

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

/// An element: name, attributes in document order, children, text.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct El {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<El>,
    pub text: String,
}

impl El {
    /// An attribute's value (exact name).
    pub fn attr(&self, k: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }

    /// The first child called `name`.
    pub fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The children called `name`.
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn element(e: &BytesStart<'_>) -> El {
    El {
        name: e.name().as_ref().to_string(),
        attrs: e
            .attributes()
            .flatten()
            .filter_map(|a| {
                let v = a.normalized_value(XmlVersion::Implicit1_0).ok()?;
                Some((a.key.as_ref().to_string(), v.into_owned()))
            })
            .collect(),
        ..El::default()
    }
}

/// The predefined XML entities.
fn entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => return None,
    })
}

/// Parses `xml` into its root element.
pub(crate) fn parse(xml: &str) -> Result<El, String> {
    let mut r = Reader::from_str(xml);
    let mut stack: Vec<El> = vec![El {
        name: "#document".into(),
        ..El::default()
    }];
    loop {
        let ev = r
            .read_event()
            .map_err(|e| format!("XML error at byte {}: {e}", r.buffer_position()))?;
        match ev {
            Event::Start(e) => stack.push(element(&e)),
            Event::Empty(e) => {
                let el = element(&e);
                if let Some(top) = stack.last_mut() {
                    top.children.push(el);
                }
            }
            Event::End(_) => {
                let Some(el) = stack.pop() else {
                    return Err("unbalanced XML".into());
                };
                match stack.last_mut() {
                    Some(top) => top.children.push(el),
                    None => return Err("unbalanced XML".into()),
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.xml10_content());
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(g) => {
                let resolved = match g.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    _ => entity(&g.xml10_content()).map(str::to_string),
                };
                if let (Some(top), Some(s)) = (stack.last_mut(), resolved) {
                    top.text.push_str(&s);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 {
        return Err("the XML ends in the middle of an element".into());
    }
    stack
        .pop()
        .and_then(|d| d.children.into_iter().next())
        .ok_or_else(|| "empty XML document".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_with_entities_and_attributes() {
        let el =
            parse(r#"<?xml version="1.0"?><a x="R&amp;B"><b>Rock &amp; Roll &#233;</b><c/></a>"#)
                .unwrap();
        assert_eq!(el.name, "a");
        assert_eq!(el.attr("x"), Some("R&B"));
        assert_eq!(el.child("b").unwrap().text, "Rock & Roll é");
        assert!(el.child("c").is_some());
        assert!(parse("<a><b></a>").is_err());
    }
}
