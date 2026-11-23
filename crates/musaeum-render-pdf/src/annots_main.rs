//! The PDF annotations worker: reads a PDF's annotations, writes Musaeum's as an update to put
//! after a PDF's bytes, and reads and writes XFDF, in a process that can do nothing else.
//!
//! It loads PDFium and locks itself down before reading anything from the host, as the render
//! workers do, then answers requests one at a time on stdin and stdout (see
//! `musaeum_pdf_annot::protocol`). It is given each PDF as a read-only handle and opens nothing:
//! it never writes a file, only says what bytes the host should write, and the host writes them to
//! a copy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::process::ExitCode;

use memmap2::Mmap;
use musaeum_pdf_annot::document::{PdfError, PdfFile};
use musaeum_pdf_annot::model::{Annot, Frame};
use musaeum_pdf_annot::protocol::{
    Found, PROTOCOL_VERSION, Reader, Reply, Request, hex, read_bytes, read_frame, write_frame,
};
use musaeum_pdf_annot::read::{Reading, read_annotations};
use musaeum_pdf_annot::write::{WriteOptions, WrittenAnnot, append_annotations};
use musaeum_pdf_annot::xfdf::{MAX_XFDF_BYTES, Xfdf, read_xfdf, write_xfdf};
use musaeum_render_pdf::annots::AnnotDocument;
use musaeum_render_pdf::engine::Pdfium;
use musaeum_render_pdf::startup::load_pdfium_and_lock_down;

/// The largest PDF this worker reads.
const MAX_PDF: u64 = 4 << 30;

fn main() -> ExitCode {
    musaeum_sandbox::target::die_quietly();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pdf annotations worker: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The PDF behind a handle the host duplicated into this process, mapped read only.
fn mapped(handle: u64) -> Result<Mmap, String> {
    // SAFETY: the host duplicated this read-only handle into this process for this request, and
    // `File` becomes its only owner. A wrong value kills this process, which runs with strict
    // handle checks, and the host starts another.
    let file = unsafe { File::from_raw_handle(handle as usize as RawHandle) };
    let length = file
        .metadata()
        .map_err(|error| format!("o arquivo não pôde ser lido: {error}"))?
        .len();
    if length == 0 {
        return Err("o arquivo está vazio".to_owned());
    }
    if length > MAX_PDF {
        return Err("o arquivo é grande demais".to_owned());
    }
    // SAFETY: the view is read only and lives as long as the request. The book's file could
    // change under it, as under the render workers; the host checks that what it copies is what
    // the library knows, by its hash, and checks the copy it writes.
    unsafe { Mmap::map(&file) }.map_err(|error| format!("o arquivo não pôde ser lido: {error}"))
}

fn describe(error: &PdfError) -> String {
    match error {
        PdfError::Encrypted => {
            "o PDF é cifrado; anotações só são gravadas em PDFs sem cifra".to_owned()
        }
        other => format!("a estrutura do PDF não pôde ser lida com segurança: {other}"),
    }
}

/// The pages whose objects name annotations, when this crate's reader can tell: the only ones
/// PDFium needs to load to read them.
fn pages_with_annotations(bytes: &[u8]) -> Option<BTreeSet<u32>> {
    let file = PdfFile::open_any(bytes).ok()?;
    file.pages_with_annotations().ok()
}

/// Every page's frame as this crate's reader sees them, when it can.
fn object_frames(bytes: &[u8]) -> Vec<Frame> {
    PdfFile::open_any(bytes)
        .ok()
        .and_then(|file| file.pages().ok())
        .map(|pages| pages.iter().map(|page| page.frame).collect())
        .unwrap_or_default()
}

fn read(pdfium: &Pdfium, bytes: &[u8], with: Reader) -> Result<Found, String> {
    let objects = PdfFile::open(bytes);
    let not_writable = objects.as_ref().err().map(describe);
    let encrypted = matches!(objects, Err(PdfError::Encrypted));
    if with != Reader::Pdfium
        && let Ok(file) = &objects
    {
        match read_annotations(file) {
            Ok(reading) => {
                let id = file.id().map(|(first, second)| (hex(&first), hex(&second)));
                if with == Reader::Objects {
                    return Ok(Found {
                        reading,
                        id,
                        not_writable,
                        ..Found::default()
                    });
                }
                // Places are only kept when PDFium places the pages the same way.
                let document = AnnotDocument::open(pdfium, bytes)?;
                let pages: BTreeSet<u32> = reading.annots.iter().map(|annot| annot.page).collect();
                let frames = document.frames(&pages);
                let agrees = document.page_count() as usize == reading.frames.len()
                    && pages.iter().all(|page| {
                        frames
                            .get(page)
                            .is_some_and(|frame| frame.same_as(&reading.frames[*page as usize]))
                    });
                if agrees {
                    return Ok(Found {
                        reading,
                        id,
                        not_writable,
                        ..Found::default()
                    });
                }
            }
            Err(error) if with == Reader::Objects => return Err(describe(&error)),
            Err(_) => {}
        }
    } else if with == Reader::Objects {
        return Err(not_writable.unwrap_or_default());
    }
    let document = AnnotDocument::open(pdfium, bytes)?;
    let only = pages_with_annotations(bytes);
    let (reading, counts) = document.read(only.as_ref(), &object_frames(bytes));
    Ok(Found {
        reading,
        id: document.file_id(),
        encrypted,
        not_writable,
        counts,
        ..Found::default()
    })
}

fn export(
    pdfium: &Pdfium,
    bytes: &[u8],
    annots: &[Annot],
    options: &WriteOptions,
) -> Result<(Vec<u8>, Vec<WrittenAnnot>), String> {
    let file = PdfFile::open(bytes).map_err(|error| describe(&error))?;
    // The same check PDFium makes before it saves: what it would not open, nothing is written to.
    let pages = file.pages().map_err(|error| describe(&error))?;
    let touched: BTreeSet<u32> = annots.iter().map(|annot| annot.page).collect();
    if let Some(page) = touched.iter().find(|page| **page as usize >= pages.len()) {
        return Err(format!("o PDF não tem a página {}", page + 1));
    }
    let document = AnnotDocument::open(pdfium, bytes)?;
    if document.page_count() as usize != pages.len() {
        return Err(format!(
            "o PDFium vê {} páginas e a estrutura do arquivo, {}: nada é gravado num arquivo assim",
            document.page_count(),
            pages.len()
        ));
    }
    let frames = document.frames(&touched);
    for page in &touched {
        let ours = pages[*page as usize].frame;
        if !frames.get(page).is_some_and(|frame| frame.same_as(&ours)) {
            return Err(format!(
                "a caixa da página {} não é a que o PDFium mostra: nada é gravado nela",
                page + 1
            ));
        }
    }
    let update = append_annotations(&file, annots, options).map_err(|error| describe(&error))?;
    Ok((update.tail, update.written))
}

/// A frame for every page: PDFium's for `pages`, and this crate's reader's or US Letter for the
/// others, which nothing is placed on.
fn frames_for(
    document: &AnnotDocument<'_, '_>,
    bytes: &[u8],
    pages: &BTreeSet<u32>,
) -> (Vec<Frame>, BTreeMap<u32, Frame>) {
    let loaded = document.frames(pages);
    let objects = object_frames(bytes);
    let all = (0..document.page_count())
        .map(|index| {
            loaded
                .get(&index)
                .or(objects.get(index as usize))
                .copied()
                .unwrap_or_else(|| Frame::of(None, None, 0))
        })
        .collect();
    (all, loaded)
}

fn read_xfdf_against(pdfium: &Pdfium, bytes: &[u8], xfdf: &[u8]) -> Result<Found, String> {
    let xfdf = read_xfdf(xfdf).map_err(|error| format!("o XFDF foi recusado: {error}"))?;
    let document = AnnotDocument::open(pdfium, bytes)?;
    let count = document.page_count();
    let pages: BTreeSet<u32> = xfdf
        .annots
        .iter()
        .map(|annot| annot.page)
        .filter(|page| *page < count)
        .collect();
    let (frames, loaded) = frames_for(&document, bytes, &pages);
    let mut skipped = xfdf.skipped;
    let mut annots = Vec::with_capacity(xfdf.annots.len());
    for annot in xfdf.annots {
        match loaded.get(&annot.page).filter(|frame| frame.is_usable()) {
            Some(frame) => annots.push(annot.to_uv(frame)),
            None => *skipped.entry("(no such page)".to_owned()).or_default() += 1,
        }
    }
    Ok(Found {
        reading: Reading {
            frames,
            annots,
            skipped,
        },
        id: document.file_id(),
        xfdf_ids: xfdf.ids,
        ..Found::default()
    })
}

fn write_xfdf_for(
    pdfium: &Pdfium,
    bytes: &[u8],
    annots: &[Annot],
    href: Option<String>,
) -> Result<Vec<u8>, String> {
    let document = AnnotDocument::open(pdfium, bytes)?;
    let pages: BTreeSet<u32> = annots.iter().map(|annot| annot.page).collect();
    let frames = document.frames(&pages);
    let mut placed = Vec::with_capacity(annots.len());
    for annot in annots {
        let Some(frame) = frames.get(&annot.page).filter(|frame| frame.is_usable()) else {
            return Err(format!("o PDF não tem a página {}", annot.page + 1));
        };
        placed.push(annot.to_user(frame));
    }
    let xfdf = Xfdf {
        href,
        ids: document.file_id(),
        annots: placed,
        skipped: BTreeMap::new(),
    };
    Ok(write_xfdf(&xfdf).into_bytes())
}

fn send_bytes(
    output: &mut impl Write,
    bytes: &[u8],
    written: Vec<WrittenAnnot>,
    base: u64,
) -> io::Result<()> {
    write_frame(
        output,
        &Reply::Bytes {
            length: bytes.len() as u64,
            written,
            base,
        },
    )?;
    output.write_all(bytes)?;
    output.flush()
}

fn run() -> io::Result<()> {
    // Before reading anything from the host, so nothing from a file ever runs with more than this.
    let pdfium = load_pdfium_and_lock_down()
        .map_err(|error| io::Error::other(format!("{}: {}", error.context, error.message)))?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    write_frame(
        &mut output,
        &Reply::Ready {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    while let Some(request) = read_frame::<Request>(&mut input)? {
        match request {
            Request::Read { handle, with } => {
                let reply = match mapped(handle).and_then(|map| read(&pdfium, &map, with)) {
                    Ok(found) => Reply::Found(found),
                    Err(reason) => Reply::Failed { reason },
                };
                write_frame(&mut output, &reply)?;
            }
            Request::Export {
                handle,
                annots,
                options,
            } => match mapped(handle).and_then(|map| {
                export(&pdfium, &map, &annots, &options).map(|done| (done, map.len() as u64))
            }) {
                Ok(((tail, written), base)) => send_bytes(&mut output, &tail, written, base)?,
                Err(reason) => write_frame(&mut output, &Reply::Failed { reason })?,
            },
            Request::ReadXfdf { handle, bytes } => {
                // Read off the pipe whatever happens to the PDF, so the conversation stays in step.
                let xfdf = read_bytes(&mut input, bytes, MAX_XFDF_BYTES as u64)?;
                let reply =
                    match mapped(handle).and_then(|map| read_xfdf_against(&pdfium, &map, &xfdf)) {
                        Ok(found) => Reply::Found(found),
                        Err(reason) => Reply::Failed { reason },
                    };
                write_frame(&mut output, &reply)?;
            }
            Request::WriteXfdf {
                handle,
                annots,
                href,
            } => {
                match mapped(handle).and_then(|map| write_xfdf_for(&pdfium, &map, &annots, href)) {
                    Ok(bytes) => send_bytes(&mut output, &bytes, Vec::new(), 0)?,
                    Err(reason) => write_frame(&mut output, &Reply::Failed { reason })?,
                }
            }
        }
    }
    Ok(())
}
