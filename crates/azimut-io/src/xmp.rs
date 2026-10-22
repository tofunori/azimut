//! Minimal XMP reader for the DJI `drone-dji:` fields Azimut needs.
//!
//! DJI writes most fields as attributes of `rdf:Description`, but some tools rewrite them as
//! child elements. Both forms are read. Fields are looked up by local name (after the
//! namespace prefix), so `drone-dji:BandName` and `Camera:BandName` are both found.

use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};

/// Flat view of the fields found in one or more XMP packets.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct XmpFields {
    /// Qualified name (e.g. `drone-dji:CaptureUUID`) to value.
    pub qualified: HashMap<String, String>,
}

impl XmpFields {
    /// Parse every packet. Malformed packets are read as far as possible.
    pub fn parse<S: AsRef<str>>(packets: &[S]) -> Self {
        let mut out = XmpFields::default();
        for p in packets {
            parse_packet(p.as_ref(), &mut out.qualified);
        }
        out
    }

    /// Value for a qualified name, e.g. `drone-dji:UTCAtExposure`.
    pub fn get(&self, qualified: &str) -> Option<&str> {
        self.qualified.get(qualified).map(String::as_str)
    }

    /// First value whose local name matches, preferring the `drone-dji` namespace.
    pub fn get_local(&self, local: &str) -> Option<&str> {
        let dji = format!("drone-dji:{local}");
        if let Some(v) = self.qualified.get(&dji) {
            return Some(v);
        }
        let mut keys: Vec<&String> = self
            .qualified
            .keys()
            .filter(|k| k.rsplit(':').next() == Some(local))
            .collect();
        keys.sort(); // deterministic choice
        keys.first()
            .and_then(|k| self.qualified.get(*k))
            .map(String::as_str)
    }
}

fn parse_packet(xml: &str, out: &mut HashMap<String, String>) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<String> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                read_attributes(&e, out);
                stack.push(String::from_utf8_lossy(e.name().as_ref()).into_owned());
            }
            Ok(Event::Empty(e)) => read_attributes(&e, out),
            Ok(Event::Text(t)) => {
                if let Some(name) = stack.last() {
                    if is_field_name(name) {
                        if let Ok(v) = t.unescape() {
                            let v = v.trim();
                            if !v.is_empty() {
                                out.entry(name.clone()).or_insert_with(|| v.to_string());
                            }
                        }
                    }
                }
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break, // keep what was read so far
        }
    }
}

/// Element and attribute names that carry data, not RDF structure.
fn is_field_name(name: &str) -> bool {
    name.contains(':') && !name.starts_with("rdf:") && !name.starts_with("x:") && !name.starts_with("xmlns")
}

fn read_attributes(e: &quick_xml::events::BytesStart<'_>, out: &mut HashMap<String, String>) {
    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        if !is_field_name(&key) {
            continue;
        }
        if let Ok(v) = attr.unescape_value() {
            out.entry(key).or_insert_with(|| v.trim().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:drone-dji="http://www.dji.com/drone-dji/1.0/"
    drone-dji:UTCAtExposure="2023:03:09 02:47:57.725671"
    drone-dji:CaptureUUID="0f8fad5b-d9cb-469f-a165-70867728950e"
    drone-dji:BandName="RedEdge">
   <drone-dji:DewarpData>2022-06-08;1,2,3</drone-dji:DewarpData>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn reads_attributes_and_elements() {
        let f = XmpFields::parse(&[SAMPLE]);
        assert_eq!(
            f.get("drone-dji:UTCAtExposure"),
            Some("2023:03:09 02:47:57.725671")
        );
        assert_eq!(f.get_local("BandName"), Some("RedEdge"));
        assert_eq!(f.get_local("DewarpData"), Some("2022-06-08;1,2,3"));
        assert!(f.get("rdf:about").is_none());
    }

    #[test]
    fn local_lookup_finds_other_namespaces() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="r"><rdf:Description xmlns:Camera="c" Camera:BandName="NIR"/></rdf:RDF></x:xmpmeta>"#;
        let f = XmpFields::parse(&[xml]);
        assert_eq!(f.get_local("BandName"), Some("NIR"));
    }

    #[test]
    fn several_packets_are_merged() {
        let a = r#"<x:xmpmeta xmlns:x="x"><rdf:RDF xmlns:rdf="r"><rdf:Description xmlns:drone-dji="d" drone-dji:CaptureUUID="u1"/></rdf:RDF></x:xmpmeta>"#;
        let b = r#"<x:xmpmeta xmlns:x="x"><rdf:RDF xmlns:rdf="r"><rdf:Description xmlns:drone-dji="d" drone-dji:BandName="Green"/></rdf:RDF></x:xmpmeta>"#;
        let f = XmpFields::parse(&[a, b]);
        assert_eq!(f.get_local("CaptureUUID"), Some("u1"));
        assert_eq!(f.get_local("BandName"), Some("Green"));
    }

    #[test]
    fn truncated_packet_keeps_what_was_read() {
        let xml = r#"<x:xmpmeta xmlns:x="x"><rdf:RDF xmlns:rdf="r"><rdf:Description xmlns:drone-dji="d" drone-dji:CaptureUUID="u1"/><broken"#;
        let f = XmpFields::parse(&[xml]);
        assert_eq!(f.get_local("CaptureUUID"), Some("u1"));
    }
}
