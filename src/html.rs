//! HTML parsing and XPath, 1:1 with Python ddgs (lxml 6.1.3 + libxml2 2.14.6).
//!
//! Upstream engines parse with
//! `lxml.html.fromstring(text, parser=HTMLParser(remove_blank_text=True,
//! remove_comments=True, remove_pis=True, collect_ids=False))` and then run
//! XPath strings. This module links the same libxml2 version (vendored, built
//! by `build.rs`), sets the parser up like lxml (`csrc/html_shim.c`), ports
//! `lxml.html.fromstring`'s fragment logic, and evaluates XPath with libxml2.
//! Engines can therefore use upstream's XPath strings verbatim.
//!
//! Result conversion follows lxml's `_unwrapXPathObject`: element nodes stay
//! nodes; text and attribute nodes become strings; XPath strings, numbers and
//! booleans become [`XValue::Str`], [`XValue::Num`], [`XValue::Bool`].

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Once;

use crate::{Result, WebSearchError};

#[repr(C)]
struct XmlDoc {
    _private: [u8; 0],
}
#[repr(C)]
struct XmlNode {
    _private: [u8; 0],
}
#[repr(C)]
struct XmlXPathObject {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn ws_init();
    fn ws_html_parse(buffer: *const c_char, len: c_int) -> *mut XmlDoc;
    fn ws_doc_free(doc: *mut XmlDoc);
    fn ws_doc_root(doc: *mut XmlDoc) -> *mut XmlNode;
    fn ws_node_type(node: *mut XmlNode) -> c_int;
    fn ws_node_name(node: *mut XmlNode) -> *const c_char;
    fn ws_node_children(node: *mut XmlNode) -> *mut XmlNode;
    fn ws_node_next(node: *mut XmlNode) -> *mut XmlNode;
    fn ws_node_set_name(node: *mut XmlNode, name: *const c_char);
    fn ws_node_content(node: *mut XmlNode) -> *mut c_char;
    fn ws_node_attr(node: *mut XmlNode, name: *const c_char) -> *mut c_char;
    fn ws_free(ptr: *mut c_void);
    fn ws_xpath_eval(
        doc: *mut XmlDoc,
        node: *mut XmlNode,
        expr: *const c_char,
    ) -> *mut XmlXPathObject;
    fn ws_xpath_free(obj: *mut XmlXPathObject);
    fn ws_xpath_type(obj: *mut XmlXPathObject) -> c_int;
    fn ws_xpath_nodeset_len(obj: *mut XmlXPathObject) -> c_int;
    fn ws_xpath_nodeset_item(obj: *mut XmlXPathObject, i: c_int) -> *mut XmlNode;
    fn ws_xpath_string(obj: *mut XmlXPathObject) -> *const c_char;
    fn ws_xpath_number(obj: *mut XmlXPathObject) -> c_double;
    fn ws_xpath_bool(obj: *mut XmlXPathObject) -> c_int;
}

// libxml2 xmlElementType values.
const ELEMENT_NODE: c_int = 1;
const ATTRIBUTE_NODE: c_int = 2;
const TEXT_NODE: c_int = 3;
const CDATA_SECTION_NODE: c_int = 4;
// libxml2 xmlXPathObjectType values.
const XPATH_NODESET: c_int = 1;
const XPATH_BOOLEAN: c_int = 2;
const XPATH_NUMBER: c_int = 3;
const XPATH_STRING: c_int = 4;

/// lxml.html.defs.block_tags (used by `fromstring` for fragments).
const BLOCK_TAGS: &[&str] = &[
    "address",
    "blockquote",
    "center",
    "del",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "ins",
    "isindex",
    "noscript",
    "p",
    "pre",
    "dir",
    "dl",
    "dt",
    "dd",
    "li",
    "menu",
    "ol",
    "ul",
    "table",
    "caption",
    "colgroup",
    "col",
    "thead",
    "tfoot",
    "tbody",
    "tr",
    "td",
    "th",
    "fieldset",
    "form",
    "legend",
    "optgroup",
    "option",
];

static INIT: Once = Once::new();

fn init() {
    // SAFETY: xmlInitParser is idempotent and thread-safe; Once runs it once.
    INIT.call_once(|| unsafe { ws_init() });
}

/// Copy a libxml2-owned C string. `None` for NULL.
///
/// SAFETY: `ptr` must be NULL or point to a NUL-terminated string that stays
/// valid for the duration of the call.
unsafe fn borrowed_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    Some(
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Copy and free a string that libxml2 allocated for us. `None` for NULL.
///
/// SAFETY: `ptr` must be NULL or a NUL-terminated string allocated by libxml2
/// (xmlMalloc) that nobody else frees.
unsafe fn owned_string(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    let value = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: allocated by libxml2 and owned by us.
    unsafe { ws_free(ptr.cast()) };
    Some(value)
}

/// A parsed HTML document. Not `Send`/`Sync`: libxml2 trees are not
/// thread-safe, so a document stays on the thread (and await-free scope)
/// that parsed it.
pub(crate) struct Document {
    doc: NonNull<XmlDoc>,
    /// The element `lxml.html.fromstring` returns.
    root: NonNull<XmlNode>,
    _not_send: PhantomData<*mut ()>,
}

impl Drop for Document {
    fn drop(&mut self) {
        // SAFETY: we own `doc`; every Node borrows `self`, so none outlive it.
        unsafe { ws_doc_free(self.doc.as_ptr()) };
    }
}

impl Document {
    /// `lxml.html.fromstring(html_text, parser=<ddgs parser>)`.
    ///
    /// Errors like lxml does: an empty document raises `ParserError`
    /// ("Document is empty").
    pub(crate) fn fromstring(html_text: &str) -> Result<Self> {
        init();
        let len = c_int::try_from(html_text.len())
            .map_err(|_| WebSearchError::Parse("Document is too large".to_owned()))?;
        if len == 0 {
            return Err(WebSearchError::Parse("Document is empty".to_owned()));
        }
        // SAFETY: the buffer is valid for `len` bytes during the call; libxml2
        // copies what it needs.
        let doc = unsafe { ws_html_parse(html_text.as_ptr().cast(), len) };
        let doc = NonNull::new(doc)
            .ok_or_else(|| WebSearchError::Parse("Document is empty".to_owned()))?;
        // SAFETY: `doc` is a valid document we own.
        let root = unsafe { ws_doc_root(doc.as_ptr()) };
        let Some(root) = NonNull::new(root) else {
            // SAFETY: we own `doc` and give up on it here.
            unsafe { ws_doc_free(doc.as_ptr()) };
            return Err(WebSearchError::Parse("Document is empty".to_owned()));
        };
        let mut document = Self {
            doc,
            root,
            _not_send: PhantomData,
        };
        if !looks_like_full_html(html_text) {
            document.root = document.fragment_root().unwrap_or(document.root);
        }
        Ok(document)
    }

    /// The element `fromstring` returned (`tree` in upstream code).
    pub(crate) fn root(&self) -> Node<'_> {
        Node {
            ptr: self.root,
            doc: self,
        }
    }

    /// Port of the fragment branch of `lxml.html.fromstring`.
    fn fragment_root(&mut self) -> Option<NonNull<XmlNode>> {
        let html = self.root();
        let bodies: Vec<Node<'_>> = html
            .element_children()
            .filter(|n| n.tag() == "body")
            .collect();
        let heads = html.element_children().any(|n| n.tag() == "head");
        let body = bodies.first().copied();
        // Several <body> elements cannot come out of libxml2's HTML parser,
        // which merges them; lxml's merge step is therefore not ported.
        if heads {
            return None;
        }
        let body = body?;
        let elements: Vec<Node<'_>> = body.element_children().collect();
        let text_blank = body.leading_text().is_none_or(|t| t.trim().is_empty());
        if elements.len() == 1 {
            let last = elements[0];
            let tail_blank = last.tail().is_none_or(|t| t.trim().is_empty());
            if text_blank && tail_blank {
                return Some(last.ptr);
            }
        }
        let block = body.contains_block_level_tag();
        let name = if block { c"div" } else { c"span" };
        // SAFETY: body is a valid element of our document; name is a C string.
        unsafe { ws_node_set_name(body.ptr.as_ptr(), name.as_ptr()) };
        Some(body.ptr)
    }
}

/// `_looks_like_full_html_unicode`: `^\s*<(?:html|!doctype)` (case-insensitive).
fn looks_like_full_html(text: &str) -> bool {
    // Python `\s` in a str pattern is Unicode whitespace.
    let rest =
        text.trim_start_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c));
    let lower: String = rest.chars().take(9).collect::<String>().to_lowercase();
    lower.starts_with("<html") || lower.starts_with("<!doctype")
}

/// A node of a [`Document`].
#[derive(Clone, Copy)]
pub(crate) struct Node<'d> {
    ptr: NonNull<XmlNode>,
    doc: &'d Document,
}

/// One item of an XPath node-set, converted like lxml.
pub(crate) enum XItem<'d> {
    /// An element (or other non-text) node.
    Node(Node<'d>),
    /// A text, CDATA or attribute node (lxml "smart string").
    Text(String),
}

/// An XPath result, converted like lxml.
#[allow(dead_code)] // the number/boolean payloads are read by tests only so far
pub(crate) enum XValue<'d> {
    Nodes(Vec<XItem<'d>>),
    Str(String),
    Num(f64),
    Bool(bool),
}

impl<'d> Node<'d> {
    fn node_type(&self) -> c_int {
        // SAFETY: ptr is a valid node of a live document.
        unsafe { ws_node_type(self.ptr.as_ptr()) }
    }

    /// Element tag name (lowercase for HTML).
    pub(crate) fn tag(&self) -> String {
        // SAFETY: the name pointer belongs to the node and is valid now.
        unsafe { borrowed_string(ws_node_name(self.ptr.as_ptr())) }.unwrap_or_default()
    }

    fn children(&self) -> impl Iterator<Item = Node<'d>> + use<'d> {
        let doc = self.doc;
        // SAFETY: valid node.
        let mut next = unsafe { ws_node_children(self.ptr.as_ptr()) };
        std::iter::from_fn(move || {
            let current = NonNull::new(next)?;
            // SAFETY: current is a valid node of the same document.
            next = unsafe { ws_node_next(current.as_ptr()) };
            Some(Node { ptr: current, doc })
        })
    }

    fn element_children(&self) -> impl Iterator<Item = Node<'d>> + use<'d> {
        self.children().filter(|n| n.node_type() == ELEMENT_NODE)
    }

    fn is_text(&self) -> bool {
        matches!(self.node_type(), TEXT_NODE | CDATA_SECTION_NODE)
    }

    /// lxml `.text`: the text before the first child element.
    fn leading_text(&self) -> Option<String> {
        let mut text: Option<String> = None;
        for child in self.children() {
            if child.node_type() == ELEMENT_NODE {
                break;
            }
            if child.is_text() {
                text.get_or_insert_with(String::new)
                    .push_str(&child.content());
            }
        }
        text
    }

    /// lxml `.tail`: the text right after this element.
    fn tail(&self) -> Option<String> {
        let mut text: Option<String> = None;
        // SAFETY: valid node.
        let mut next = unsafe { ws_node_next(self.ptr.as_ptr()) };
        while let Some(current) = NonNull::new(next) {
            let node = Node {
                ptr: current,
                doc: self.doc,
            };
            if !node.is_text() {
                break;
            }
            text.get_or_insert_with(String::new)
                .push_str(&node.content());
            // SAFETY: valid node.
            next = unsafe { ws_node_next(current.as_ptr()) };
        }
        text
    }

    fn contains_block_level_tag(&self) -> bool {
        // `el.iter(etree.Element)` includes the element itself.
        if BLOCK_TAGS.contains(&self.tag().as_str()) {
            return true;
        }
        self.element_children()
            .any(|c| c.contains_block_level_tag())
    }

    /// Text content of a text/CDATA/attribute node.
    fn content(&self) -> String {
        // SAFETY: valid node; the returned string is ours to free.
        unsafe { owned_string(ws_node_content(self.ptr.as_ptr())) }.unwrap_or_default()
    }

    /// lxml `element.get(name)`.
    #[allow(dead_code)] // lxml API surface; used by tests so far
    pub(crate) fn get(&self, name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        // SAFETY: valid node and C string; the result is ours to free.
        unsafe { owned_string(ws_node_attr(self.ptr.as_ptr(), name.as_ptr())) }
    }

    /// lxml `element.attrib.items()`, in document order.
    #[allow(dead_code)] // used by the parser-parity test
    pub(crate) fn attributes(&self) -> Result<Vec<(String, String)>> {
        let XValue::Num(count) = self.xpath("count(@*)")? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        // `count` is a small non-negative integer from XPath count().
        let count = count as usize;
        for i in 1..=count {
            let name = match self.xpath(&format!("name(@*[{i}])"))? {
                XValue::Str(s) => s,
                _ => String::new(),
            };
            let value = match self.xpath(&format!("string(@*[{i}])"))? {
                XValue::Str(s) => s,
                _ => String::new(),
            };
            out.push((name, value));
        }
        Ok(out)
    }

    /// lxml `element.xpath(expr)`. Errors like lxml's `XPathEvalError`.
    pub(crate) fn xpath(&self, expr: &str) -> Result<XValue<'d>> {
        let c_expr = CString::new(expr)
            .map_err(|_| WebSearchError::Parse(format!("Invalid expression: {expr}")))?;
        // SAFETY: doc and node are valid; the result object is ours to free.
        let obj =
            unsafe { ws_xpath_eval(self.doc.doc.as_ptr(), self.ptr.as_ptr(), c_expr.as_ptr()) };
        let Some(obj) = NonNull::new(obj) else {
            return Err(WebSearchError::Parse(format!("Invalid expression: {expr}")));
        };
        let obj = obj.as_ptr();
        // SAFETY: obj is a valid XPath object until ws_xpath_free below; node
        // pointers in the node-set belong to the document, not to obj.
        let value = unsafe {
            match ws_xpath_type(obj) {
                XPATH_NODESET => {
                    let len = ws_xpath_nodeset_len(obj);
                    let mut items = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
                    for i in 0..len {
                        let Some(ptr) = NonNull::new(ws_xpath_nodeset_item(obj, i)) else {
                            continue;
                        };
                        let node = Node { ptr, doc: self.doc };
                        match node.node_type() {
                            TEXT_NODE | CDATA_SECTION_NODE | ATTRIBUTE_NODE => {
                                items.push(XItem::Text(node.content()));
                            }
                            _ => items.push(XItem::Node(node)),
                        }
                    }
                    XValue::Nodes(items)
                }
                XPATH_BOOLEAN => XValue::Bool(ws_xpath_bool(obj) != 0),
                XPATH_NUMBER => XValue::Num(ws_xpath_number(obj)),
                XPATH_STRING => {
                    XValue::Str(borrowed_string(ws_xpath_string(obj)).unwrap_or_default())
                }
                other => {
                    ws_xpath_free(obj);
                    return Err(WebSearchError::Parse(format!(
                        "Unsupported XPath result type {other}"
                    )));
                }
            }
        };
        // SAFETY: we own obj and no longer use it.
        unsafe { ws_xpath_free(obj) };
        Ok(value)
    }

    /// `item.xpath(expr)` where upstream expects element results.
    pub(crate) fn xpath_nodes(&self, expr: &str) -> Result<Vec<Node<'d>>> {
        match self.xpath(expr)? {
            XValue::Nodes(items) => Ok(items
                .into_iter()
                .filter_map(|i| match i {
                    XItem::Node(n) => Some(n),
                    XItem::Text(_) => None,
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    /// `item.xpath(expr)` where upstream expects a list of strings (text()
    /// or @attr). Python would raise `TypeError` when joining a non-string;
    /// we report that as an error too.
    pub(crate) fn xpath_strings(&self, expr: &str) -> Result<Vec<String>> {
        match self.xpath(expr)? {
            XValue::Nodes(items) => items
                .into_iter()
                .map(|i| match i {
                    XItem::Text(t) => Ok(t),
                    XItem::Node(_) => Err(WebSearchError::Internal(
                        "sequence item 0: expected str instance, lxml.html.HtmlElement found"
                            .to_owned(),
                    )),
                })
                .collect(),
            XValue::Str(s) => Ok(vec![s]),
            _ => Err(WebSearchError::Internal(
                "can only join an iterable of str".to_owned(),
            )),
        }
    }

    /// `" ".join("".join(item.xpath(expr)).split())` — the per-field step of
    /// upstream `BaseSearchEngine.extract_results`.
    pub(crate) fn xpath_joined(&self, expr: &str) -> Result<String> {
        let joined = self.xpath_strings(expr)?.concat();
        Ok(crate::normalize::collapse_whitespace(&joined))
    }
}

/// Port of upstream `BaseSearchEngine.extract_results` (before the result
/// dataclass setattr): parse, select `items_xpath`, and for every item
/// compute each `elements_xpath` field. Returns one `(key, value)` list per
/// item, in `elements` order.
pub(crate) fn extract_results(
    html_text: &str,
    items_xpath: &str,
    elements: &[(&'static str, &str)],
) -> Result<Vec<Vec<(&'static str, String)>>> {
    let document = Document::fromstring(html_text)?;
    let tree = document.root();
    let mut results = Vec::new();
    for item in tree.xpath_nodes(items_xpath)? {
        let mut fields = Vec::with_capacity(elements.len());
        for (key, expr) in elements {
            fields.push((*key, item.xpath_joined(expr)?));
        }
        results.push(fields);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_runs_xpath() {
        let doc = Document::fromstring(
            "<!DOCTYPE html><html><body><div class='r'><a href='/x'>A <b>b</b></a></div>\
             <div class='r'><a href='/y'>C</a></div></body></html>",
        )
        .unwrap();
        let items = doc.root().xpath_nodes("//div[@class='r']").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].xpath_joined(".//a//text()").unwrap(), "A b");
        assert_eq!(items[1].xpath_strings("./a/@href").unwrap(), vec!["/y"]);
    }

    #[test]
    fn comments_and_pis_are_removed_like_lxml() {
        let doc =
            Document::fromstring("<html><body><p>a<!-- hidden -->b</p></body></html>").unwrap();
        assert_eq!(doc.root().xpath_joined("//p//text()").unwrap(), "ab");
        assert!(
            matches!(doc.root().xpath("//comment()").unwrap(), XValue::Nodes(v) if v.is_empty())
        );
    }

    #[test]
    fn fragment_single_element_returns_it() {
        let doc = Document::fromstring("<div id='x'><span>t</span></div>").unwrap();
        assert_eq!(doc.root().tag(), "div");
        assert_eq!(doc.root().get("id").as_deref(), Some("x"));
    }

    #[test]
    fn fragment_many_elements_renames_body() {
        let doc = Document::fromstring("<div>a</div><div>b</div>").unwrap();
        assert_eq!(doc.root().tag(), "div");
        let doc = Document::fromstring("<span>a</span><b>b</b>").unwrap();
        assert_eq!(doc.root().tag(), "span");
    }

    #[test]
    fn xpath_scalar_results() {
        let doc = Document::fromstring("<html><body><p>x</p><p>y</p></body></html>").unwrap();
        assert!(matches!(doc.root().xpath("count(//p)").unwrap(), XValue::Num(n) if n == 2.0));
        assert!(matches!(
            doc.root().xpath("boolean(//p)").unwrap(),
            XValue::Bool(true)
        ));
        assert!(matches!(doc.root().xpath("string(//p)").unwrap(), XValue::Str(s) if s == "x"));
    }

    /// Parser parity: our libxml2 tree equals lxml's for every HTML fixture.
    /// The expected dumps come from `scripts/regen_expected.py` (upstream
    /// ddgs 9.16.1, lxml 6.1.3, libxml2 2.14.6).
    #[test]
    fn parser_parity() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir.join("parser")).unwrap() {
            let path = entry.unwrap().path();
            let stem = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let expected: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let mut html = std::fs::read_to_string(dir.join(format!("{stem}.html"))).unwrap();
            if stem == "annasarchive_books_page" {
                // AnnasArchive.pre_process_html
                html = html.replace("<!--", "").replace("-->", "");
            }
            let doc = Document::fromstring(&html).unwrap();
            let root = doc.root();
            let mut nodes = Vec::new();
            let XValue::Nodes(items) = root.xpath("//node()").unwrap() else {
                panic!("node-set expected");
            };
            for item in items {
                match item {
                    XItem::Text(t) => nodes.push(serde_json::json!(["t", t])),
                    XItem::Node(n) => {
                        let attrs: Vec<serde_json::Value> = n
                            .attributes()
                            .unwrap()
                            .into_iter()
                            .map(|(k, v)| serde_json::json!([k, v]))
                            .collect();
                        nodes.push(serde_json::json!(["e", n.tag(), attrs]));
                    }
                }
            }
            let actual = serde_json::json!({"root": root.tag(), "nodes": nodes});
            if actual != expected {
                let a = actual["nodes"].as_array().unwrap();
                let e = expected["nodes"].as_array().unwrap();
                let first = a.iter().zip(e.iter()).position(|(x, y)| x != y);
                panic!(
                    "{stem}: tree differs from lxml (root {} vs {}, {} vs {} nodes, first difference at {:?}: {:?} vs {:?})",
                    actual["root"],
                    expected["root"],
                    a.len(),
                    e.len(),
                    first,
                    first.map(|i| &a[i]),
                    first.map(|i| &e[i]),
                );
            }
            checked += 1;
        }
        assert!(checked >= 13, "only {checked} parser fixtures checked");
    }

    #[test]
    fn empty_and_invalid() {
        assert!(Document::fromstring("").is_err());
        let doc = Document::fromstring("<html><body></body></html>").unwrap();
        assert!(doc.root().xpath("//[").is_err());
    }
}
