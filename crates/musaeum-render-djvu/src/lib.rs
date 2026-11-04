//! Renders DjVu documents with DjVuLibre, in a process that does nothing else.
//!
//! The binary `musaeum-render-djvu` is the worker. DjVuLibre is linked in statically and reads the
//! document from the mapped bytes the host shared; it is never given a path or a URL.

pub mod book;
