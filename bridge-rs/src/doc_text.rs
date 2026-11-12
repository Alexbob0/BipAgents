//! A document's words, for an agent that reads a file without tools of its own (shared_files.rs `read_sent_file`: a
//! shared agent's turns have no terminal, the family's agent no file tools at all): PDFs (pdf-extract), Word,
//! PowerPoint and OpenDocument text and slides (their XML inside the zip, read with quick-xml), Excel and Calc
//! spreadsheets (calamine), and plain text formats. All pure Rust, nothing sent anywhere, each read bounded in time.
//!
//! No Python counterpart.

use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

/// What reading gave.
#[derive(Debug, PartialEq)]
pub enum Read {
    Text(String),
    /// A file of a known kind with no words in it (a scanned PDF).
    NoText,
    /// A kind this can't read.
    Unreadable,
}

/// A text file read at most.
const TEXT_FILE_MAX: u64 = 2 * 1024 * 1024;
/// The XML of one part of an office file read at most (a huge one is cut).
const PART_MAX: u64 = 32 * 1024 * 1024;
/// Rows of one sheet given at most.
const ROWS_MAX: usize = 2000;
const TIMEOUT: Duration = Duration::from_secs(20);

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// The kinds read, by extension (or the MIME type for a PDF or a text).
pub fn document_text(path: &Path, name: &str, mime: &str) -> Read {
    let ext = extension(name);
    let bytes = || std::fs::read(path).ok();
    let found = match ext.as_str() {
        _ if ext == "pdf" || mime == "application/pdf" => bytes().map(|b| bounded(move || pdf(&b))),
        "docx" | "docm" | "dotx" => bytes().map(|b| bounded(move || docx(&b))),
        "pptx" | "pptm" | "ppsx" => bytes().map(|b| bounded(move || pptx(&b))),
        "odt" | "odp" | "ott" => bytes().map(|b| bounded(move || odf(&b))),
        "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" => bytes().map(|b| bounded(move || sheets(b))),
        _ if is_text(&ext, mime) => return text_file(path, &ext, mime),
        _ => return Read::Unreadable,
    };
    match found.flatten() {
        Some(text) if !text.trim().is_empty() => Read::Text(squeeze(&text)),
        Some(_) => Read::NoText,
        None => Read::Unreadable,
    }
}

fn is_text(ext: &str, mime: &str) -> bool {
    mime.starts_with("text/")
        || ["txt", "md", "markdown", "csv", "tsv", "json", "xml", "html", "htm", "eml", "ics", "vcf", "log", "rtf"]
            .contains(&ext)
}

fn text_file(path: &Path, ext: &str, mime: &str) -> Read {
    let Ok(file) = std::fs::File::open(path) else { return Read::Unreadable };
    let mut bytes = Vec::new();
    if file.take(TEXT_FILE_MAX).read_to_end(&mut bytes).is_err() {
        return Read::Unreadable;
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let text =
        if ["html", "htm"].contains(&ext) || mime == "text/html" { crate::mailbox::html_text(&text) } else { text };
    if text.trim().is_empty() {
        Read::NoText
    } else {
        Read::Text(squeeze(&text))
    }
}

/// `read` on its own thread, `TIMEOUT` at most, a panic of a reader taken as nothing read.
fn bounded(read: impl FnOnce() -> Option<String> + Send + 'static) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).ok().flatten();
        let _ = tx.send(found);
    });
    rx.recv_timeout(TIMEOUT).ok().flatten()
}

fn pdf(bytes: &[u8]) -> Option<String> {
    pdf_extract::extract_text_from_mem(bytes).ok()
}

/// One part of a zip (an office file), as text.
fn part(zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> Option<String> {
    let entry = zip.by_name(name).ok()?;
    let mut xml = String::new();
    entry.take(PART_MAX).read_to_string(&mut xml).ok()?;
    Some(xml)
}

fn docx(bytes: &[u8]) -> Option<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    Some(xml_words(&part(&mut zip, "word/document.xml")?, Some("t")))
}

fn pptx(bytes: &[u8]) -> Option<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    // ppt/slides/slide<N>.xml, in their order
    let mut slides: Vec<(u32, String)> = zip
        .file_names()
        .filter_map(|n| {
            let n = n.ok()?;
            let number = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
            Some((number, n.to_string()))
        })
        .collect();
    slides.sort();
    let mut out = String::new();
    for (number, name) in slides {
        let words = xml_words(&part(&mut zip, &name)?, Some("t"));
        if !words.trim().is_empty() {
            out.push_str(&format!("--- Diapositive {number} ---\n{}\n\n", words.trim()));
        }
    }
    Some(out)
}

fn odf(bytes: &[u8]) -> Option<String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    Some(xml_words(&part(&mut zip, "content.xml")?, None))
}

/// Excel and Calc: each sheet under its name, a row a line, its cells apart by tabs.
fn sheets(bytes: Vec<u8>) -> Option<String> {
    use calamine::Reader;
    let mut book = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes)).ok()?;
    let mut out = String::new();
    for name in book.sheet_names().to_vec() {
        let Ok(range) = book.worksheet_range(&name) else { continue };
        out.push_str(&format!("--- {name} ---\n"));
        for row in range.rows().take(ROWS_MAX) {
            let cells: Vec<String> = row.iter().map(|c| c.to_string()).collect();
            let line = cells.join("\t");
            if !line.trim().is_empty() {
                out.push_str(line.trim_end());
                out.push('\n');
            }
        }
        out.push('\n');
    }
    Some(out)
}

/// The words of an office XML part: a paragraph a line (`p`, `h`), tabs and breaks kept; `only_in`: the element the
/// words are in (`t` for Word and PowerPoint), else every text (OpenDocument).
pub fn xml_words(xml: &str, only_in: Option<&str>) -> String {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = String::new();
    let mut inside = 0usize;
    let words = |inside: usize| only_in.is_none() || inside > 0;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if Some(e.local_name().as_ref()) == only_in {
                    inside += 1;
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name();
                if Some(name.as_ref()) == only_in {
                    inside = inside.saturating_sub(1);
                }
                if matches!(name.as_ref(), "p" | "h") {
                    out.push('\n');
                }
            }
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "tab" => out.push('\t'),
                "br" | "line-break" | "cr" => out.push('\n'),
                "s" => out.push(' '),
                "p" | "h" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if words(inside) => out.push_str(&t.xml10_content()),
            Ok(Event::CData(t)) if words(inside) => out.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if words(inside) => match r.resolve_char_ref() {
                Ok(Some(c)) => out.push(c),
                _ => out.push_str(match &*r {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => "",
                }),
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Lines trimmed at their end, no more than one blank line in a row.
pub fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 || out.is_empty() {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;

    /// A zip of `parts` (name, content), as an office file is.
    pub fn zipped(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            let options = zip::write::SimpleFileOptions::default();
            for (name, content) in parts {
                zip.start_file(*name, options).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    pub const DOCX: &str = r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:body>
        <w:p><w:r><w:t>Inscription au club de foot</w:t></w:r></w:p>
        <w:p><w:r><w:t xml:space="preserve">Nom : Zoé </w:t></w:r><w:r><w:tab/><w:t>Né le 3 &amp; 4 mai</w:t></w:r></w:p>
        <w:p><w:r><w:instrText>PAGE</w:instrText></w:r></w:p></w:body></w:document>"#;

    /// A minimal xlsx: one sheet, inline strings and a number.
    pub fn xlsx() -> Vec<u8> {
        zipped(&[
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Cotisation</t></is></c><c r="B1"><v>120</v></c></row></sheetData></worksheet>"#,
            ),
        ])
    }

    #[test]
    fn word_powerpoint_opendocument_and_excel_give_their_words() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let docx = write("a.docx", &zipped(&[("word/document.xml", DOCX)]));
        assert_eq!(
            document_text(&docx, "inscription.docx", "application/octet-stream"),
            Read::Text("Inscription au club de foot\nNom : Zoé \tNé le 3 & 4 mai".into())
        );
        let slide = |t: &str| format!(r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:sld>"#);
        let pptx = write(
            "b.pptx",
            &zipped(&[("ppt/slides/slide2.xml", &slide("Fin")), ("ppt/slides/slide1.xml", &slide("Sortie scolaire"))]),
        );
        assert_eq!(
            document_text(&pptx, "sortie.pptx", ""),
            Read::Text("--- Diapositive 1 ---\nSortie scolaire\n\n--- Diapositive 2 ---\nFin".into())
        );
        let odt = write(
            "c.odt",
            &zipped(&[(
                "content.xml",
                r#"<office:document-content xmlns:office="o" xmlns:text="t"><office:body><office:text><text:h>Menu</text:h><text:p>Soupe<text:s/>et<text:tab/>tarte</text:p></office:text></office:body></office:document-content>"#,
            )]),
        );
        assert_eq!(document_text(&odt, "menu.odt", ""), Read::Text("Menu\nSoupe et\ttarte".into()));
        let xlsx = write("d.xlsx", &xlsx());
        assert_eq!(document_text(&xlsx, "budget.xlsx", ""), Read::Text("--- Budget ---\nCotisation\t120".into()));
        // not an office file after all, a kind it doesn't know, an empty text
        let broken = write("e.docx", b"PK\x03\x04 broken");
        assert_eq!(document_text(&broken, "e.docx", ""), Read::Unreadable);
        let zip = write("f.zip", b"PK");
        assert_eq!(document_text(&zip, "photos.zip", "application/zip"), Read::Unreadable);
        let txt = write("g.txt", b"Inscription\n\n\n\nSamedi 10 h   \n");
        assert_eq!(document_text(&txt, "g.txt", "text/plain"), Read::Text("Inscription\n\nSamedi 10 h".into()));
        let empty = write("h.pdf", b"%PDF-1.4 nothing");
        assert!(matches!(document_text(&empty, "h.pdf", "application/pdf"), Read::Unreadable | Read::NoText));
    }
}
