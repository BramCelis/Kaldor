//! Export from the Kaldor library to rekordbox.
//!
//! Output A: a rekordbox XML `collection.xml` following
//! AlphaTheta's published "rekordbox XML format" ([`rekordbox_xml`]).
//! Outputs B and C: a rekordbox USB stick with the Device Library and
//! OneLibrary ([`usb`], written by baken-export from that XML).

pub mod rekordbox_xml;
pub mod usb;
pub mod volume;

pub use rekordbox_xml::{ExportError, ExportReport, XmlExportOptions, export_xml, file_location};
pub use usb::{
    CancelToken, Progress, UsbError, UsbExportOptions, UsbFormat, UsbPlan, UsbReport, export_usb,
    plan_from_xml, plan_usb, write_plan_xml,
};

/// File name used when the export target is a folder.
pub const DEFAULT_XML_NAME: &str = "kaldor.xml";

/// Where to write the XML for a path the user typed: surrounding quotes
/// (Explorer's "Copy as path") are removed, and a folder (an existing one,
/// or a path ending in a separator) gets [`DEFAULT_XML_NAME`] inside it.
/// (a folder path failed with "Access is denied (os error 5)",
/// Windows' error for opening a folder as a file.)
pub fn xml_output_path(typed: &str) -> std::path::PathBuf {
    let t = typed.trim();
    let t = t
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t)
        .trim();
    let p = std::path::PathBuf::from(t);
    if p.is_dir() || t.ends_with(['/', '\\']) {
        p.join(DEFAULT_XML_NAME)
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_and_quoted_paths() {
        let dir = std::env::temp_dir();
        assert_eq!(
            xml_output_path(&dir.display().to_string()),
            dir.join(DEFAULT_XML_NAME)
        );
        let quoted = format!("  \"{}\"  ", dir.join("x.xml").display());
        assert_eq!(xml_output_path(&quoted), dir.join("x.xml"));
        assert_eq!(
            xml_output_path("no/such/folder/"),
            std::path::PathBuf::from("no/such/folder/").join(DEFAULT_XML_NAME)
        );
        assert_eq!(
            xml_output_path("out.xml"),
            std::path::PathBuf::from("out.xml")
        );
    }
}
