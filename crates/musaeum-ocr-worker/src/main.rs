//! The OCR worker: Tesseract in a process that can do nothing else.
//!
//! It locks itself down before reading anything from the host, then answers requests one at a
//! time on stdin and stdout: models to load, as bytes, and pictures to read. Pixels from a book are
//! hostile input, so this is the only process that ever hands them to Tesseract.

use std::io::{self, Write};
use std::process::ExitCode;

use musaeum_ocr::protocol::{
    MAX_MODEL, MAX_MODELS, MAX_PIXELS, Mode, PROTOCOL_VERSION, Reply, Request, Word, is_language,
    picture_fits, read_bytes, read_frame, write_frame,
};
use musaeum_tesseract::{Engine, GrayImage, Segmentation, add_model};

fn main() -> ExitCode {
    musaeum_sandbox::target::die_quietly();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ocr worker: {error}");
            ExitCode::FAILURE
        }
    }
}

fn segmentation(mode: Mode) -> Segmentation {
    match mode {
        Mode::Auto => Segmentation::Auto,
        Mode::Block => Segmentation::Block,
        Mode::Line => Segmentation::Line,
        Mode::Sparse => Segmentation::Sparse,
    }
}

fn run() -> io::Result<()> {
    // Before reading anything from the host, so nothing from a book ever runs with more than this.
    musaeum_sandbox::target::enter_lockdown()?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    write_frame(
        &mut output,
        &Reply::Ready {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    let mut engine: Option<Engine> = None;
    while let Some(request) = read_frame::<Request>(&mut input)? {
        let reply = match request {
            Request::Load { models, languages } => {
                if models.len() > MAX_MODELS {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "too many models",
                    ));
                }
                // Every model is read off the pipe even if one is refused, so the conversation
                // stays in step.
                let mut problem = None;
                for model in &models {
                    let bytes = read_bytes(&mut input, model.bytes, MAX_MODEL)?;
                    if !is_language(&model.language) {
                        problem = Some(format!("{:?} is no language", model.language));
                    } else if let Err(error) = add_model(&model.language, &bytes) {
                        problem = Some(error);
                    }
                }
                engine = None;
                match problem {
                    Some(reason) => Reply::Failed { reason },
                    None => match Engine::new(&languages) {
                        Ok(made) => {
                            engine = Some(made);
                            Reply::Loaded
                        }
                        Err(reason) => Reply::Failed { reason },
                    },
                }
            }
            Request::Recognize {
                width,
                height,
                mode,
                ppi,
            } => {
                if !picture_fits(width, height) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a picture too large",
                    ));
                }
                let pixels =
                    read_bytes(&mut input, u64::from(width) * u64::from(height), MAX_PIXELS)?;
                match engine.as_mut() {
                    None => Reply::Failed {
                        reason: "no models were loaded".to_owned(),
                    },
                    Some(engine) => match engine.recognize(
                        GrayImage {
                            width,
                            height,
                            pixels: &pixels,
                        },
                        segmentation(mode),
                        ppi,
                    ) {
                        Ok(words) => Reply::Words(
                            words
                                .into_iter()
                                .map(|word| Word {
                                    text: word.text,
                                    left: word.left,
                                    top: word.top,
                                    right: word.right,
                                    bottom: word.bottom,
                                    confidence: word.confidence,
                                    block: word.block,
                                    paragraph: word.paragraph,
                                    line: word.line,
                                })
                                .collect(),
                        ),
                        Err(reason) => Reply::Failed { reason },
                    },
                }
            }
        };
        write_frame(&mut output, &reply)?;
        output.flush()?;
    }
    Ok(())
}
