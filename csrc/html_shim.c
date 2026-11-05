/*
 * Thin C shim over the vendored libxml2 (vendor/libxml2, v2.14.6).
 *
 * It reproduces exactly how lxml 6.1.3 (the HTML parser used by Python ddgs)
 * parses an HTML string with
 *   lxml.etree.HTMLParser(remove_blank_text=True, remove_comments=True,
 *                         remove_pis=True, collect_ids=False)
 * and evaluates XPath on the result, so that the Rust engines can use the
 * upstream XPath strings verbatim and get the same tree.
 *
 * lxml references (src/lxml/parser.pxi, lxml 6.1.3):
 *   _newParserCtxt            -> htmlCreateMemoryParserCtxt("dummy", 5)
 *   _registerHtmlErrorHandler -> SAX2 magic + structured error handler
 *   _configureSaxContext      -> sax->comment = NULL, sax->processingInstruction = NULL
 *   _HTML_DEFAULT_PARSE_OPTIONS | NOBLANKS -> RECOVER | NONET | COMPACT | NOBLANKS
 *   _parseUnicodeDoc          -> htmlCtxtReadMemory(ctxt, buf, len, NULL, enc, opts)
 */

#include <stddef.h>

#include <libxml/HTMLparser.h>
#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/xmlerror.h>
#include <libxml/xpath.h>
#include <libxml/xpathInternals.h>

/* lxml collects errors in its own log; nothing reaches stderr. */
static void ws_silent_error(void *ctx, const xmlError *error) {
    (void)ctx;
    (void)error;
}

void ws_init(void) { xmlInitParser(); }

/* Parse `len` bytes of UTF-8 HTML the way lxml's HTMLParser does. */
xmlDocPtr ws_html_parse(const char *buffer, int len) {
    htmlParserCtxtPtr ctxt;
    xmlSAXHandler *sax;
    xmlDocPtr doc;
    int options = HTML_PARSE_RECOVER | HTML_PARSE_NONET | HTML_PARSE_COMPACT |
                  HTML_PARSE_NOBLANKS;

    ctxt = htmlCreateMemoryParserCtxt("dummy", 5);
    if (ctxt == NULL) return NULL;

    sax = ctxt->sax;
    if (sax != NULL && sax->initialized && sax->initialized != XML_SAX2_MAGIC) {
        /* In 2.14 htmlInitParserCtxt mallocs the context's own handler and
         * xmlSAX2InitHtmlDefaultSAXHandler sets initialized = 1, so this
         * branch always runs and modifies the context's copy (lxml only
         * copies first when sax points at the global htmlDefaultSAXHandler). */
        sax->initialized = XML_SAX2_MAGIC;
        sax->serror = ws_silent_error;
        sax->startElementNs = NULL;
        sax->endElementNs = NULL;
        sax->_private = NULL;
    }
    if (sax != NULL) {
        sax->comment = NULL;               /* remove_comments=True */
        sax->processingInstruction = NULL; /* remove_pis=True */
    }
    doc = htmlCtxtReadMemory(ctxt, buffer, len, NULL, "UTF-8", options);
    htmlFreeParserCtxt(ctxt);
    return doc;
}

void ws_doc_free(xmlDocPtr doc) {
    if (doc != NULL) xmlFreeDoc(doc);
}

xmlNodePtr ws_doc_root(xmlDocPtr doc) { return xmlDocGetRootElement(doc); }

/* ---- nodes ---------------------------------------------------------- */

int ws_node_type(xmlNodePtr node) { return node ? (int)node->type : 0; }
const xmlChar *ws_node_name(xmlNodePtr node) { return node ? node->name : NULL; }
xmlNodePtr ws_node_children(xmlNodePtr node) { return node ? node->children : NULL; }
xmlNodePtr ws_node_next(xmlNodePtr node) { return node ? node->next : NULL; }
xmlNodePtr ws_node_parent(xmlNodePtr node) { return node ? node->parent : NULL; }
void ws_node_set_name(xmlNodePtr node, const char *name) {
    if (node) xmlNodeSetName(node, (const xmlChar *)name);
}

/* Text of a text/CDATA node, or value of an attribute node (caller frees). */
xmlChar *ws_node_content(xmlNodePtr node) {
    if (node == NULL) return NULL;
    if (node->type == XML_ATTRIBUTE_NODE)
        return xmlNodeListGetString(node->doc, node->children, 1);
    return xmlNodeGetContent(node);
}

/* Attribute value of an element (caller frees), NULL if absent. */
xmlChar *ws_node_attr(xmlNodePtr node, const char *name) {
    if (node == NULL) return NULL;
    return xmlGetNoNsProp(node, (const xmlChar *)name);
}

void ws_free(void *ptr) {
    if (ptr != NULL) xmlFree(ptr);
}

/* ---- XPath ------------------------------------------------------------ */

/* Evaluate `expr` with `node` as the context node (like lxml element.xpath). */
xmlXPathObjectPtr ws_xpath_eval(xmlDocPtr doc, xmlNodePtr node, const char *expr) {
    xmlXPathContextPtr xctxt;
    xmlXPathObjectPtr result;

    xctxt = xmlXPathNewContext(doc);
    if (xctxt == NULL) return NULL;
    xmlXPathSetErrorHandler(xctxt, ws_silent_error, NULL);
    xctxt->node = node;
    result = xmlXPathEvalExpression((const xmlChar *)expr, xctxt);
    xmlXPathFreeContext(xctxt);
    return result;
}

void ws_xpath_free(xmlXPathObjectPtr obj) {
    if (obj != NULL) xmlXPathFreeObject(obj);
}

int ws_xpath_type(xmlXPathObjectPtr obj) { return obj ? (int)obj->type : 0; }

int ws_xpath_nodeset_len(xmlXPathObjectPtr obj) {
    if (obj == NULL || obj->nodesetval == NULL) return 0;
    return obj->nodesetval->nodeNr;
}

xmlNodePtr ws_xpath_nodeset_item(xmlXPathObjectPtr obj, int i) {
    if (obj == NULL || obj->nodesetval == NULL) return NULL;
    if (i < 0 || i >= obj->nodesetval->nodeNr) return NULL;
    return obj->nodesetval->nodeTab[i];
}

const xmlChar *ws_xpath_string(xmlXPathObjectPtr obj) { return obj ? obj->stringval : NULL; }
double ws_xpath_number(xmlXPathObjectPtr obj) { return obj ? obj->floatval : 0.0; }
int ws_xpath_bool(xmlXPathObjectPtr obj) { return obj ? obj->boolval : 0; }
