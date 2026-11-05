//! Public result types.
//!
//! Each struct mirrors the matching Python `ddgs` dataclass in `results.py`:
//! same field names and field order (the JSON key order). Dataclass defaults
//! are `""` (`{}` for the video maps).
//!
//! Upstream engines that copy JSON keys with `item.get(key)` store `None` for a
//! missing key, which Python serializes as `null`. Those fields are
//! `Option<String>` here (`None` serializes as `null`). Upstream also copies some
//! JSON values verbatim whatever their type (DuckDuckGo image `height`/`width`
//! are integers; video `images`/`statistics` are objects with integer or `null`
//! values). Those fields are [`serde_json::Value`], so the output keeps the same
//! JSON types. Fields that upstream always fills with a string stay `String`.
//!
//! Upstream normalizes a field whenever a truthy value is assigned
//! (`BaseResult.__setattr__`). Here the constructors end in `normalized()`,
//! which applies the same field map:
//!
//! | fields | normalizer |
//! |--------|------------|
//! | `title`, `body`, `author`, `publisher`, `info` | strip tags, unescape entities, NFC, drop `C*` chars, collapse whitespace |
//! | `href`, `url`, `thumbnail`, `image` | percent-decode, then `" "` → `"+"` |
//!
//! `date` is normalized by the engine that produces it (an integer unix
//! timestamp becomes an ISO-8601 UTC string; strings pass through unchanged).
//! Fields not in the map (e.g. video `description`, `content`, `embed_url`) are
//! stored verbatim, as upstream does.
//!
//! **`normalized()` is not idempotent.** A second pass decodes again
//! (`"&amp;lt;"` → `"&lt;"` → `"<"`, `"%2520"` → `"%20"` → `" "`), so call it
//! once, on raw values. Upstream normalizes again only where it re-assigns a
//! field.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::normalize::{normalize_text, normalize_url};

/// Normalize a text-mapped field in place (only a non-empty value, like
/// upstream's `if value and ...`).
fn text(field: &mut String) {
    if !field.is_empty() {
        *field = normalize_text(field);
    }
}

/// Normalize a URL-mapped field in place (only a non-empty value).
fn url(field: &mut String) {
    if !field.is_empty() {
        *field = normalize_url(field);
    }
}

/// [`text`] for a field that may hold upstream's `None`.
fn text_opt(field: &mut Option<String>) {
    if let Some(value) = field {
        text(value);
    }
}

/// [`url`] for a field that may hold upstream's `None`.
fn url_opt(field: &mut Option<String>) {
    if let Some(value) = field {
        url(value);
    }
}

/// The dataclass default `""` for an optional field.
fn empty() -> Option<String> {
    Some(String::new())
}

/// A text-search result (`ddgs.results.TextResult`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextResult {
    /// Result title.
    pub title: String,
    /// Destination URL.
    pub href: String,
    /// Snippet/body text.
    pub body: String,
}

impl TextResult {
    /// Construct a text result from raw values, applying upstream field
    /// normalization.
    pub fn new(title: impl Into<String>, href: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            href: href.into(),
            body: body.into(),
        }
        .normalized()
    }

    /// Apply upstream field normalization to every mapped field.
    ///
    /// Not idempotent: use it once, on raw values (see the module docs).
    pub fn normalized(mut self) -> Self {
        text(&mut self.title);
        url(&mut self.href);
        text(&mut self.body);
        self
    }
}

/// An image-search result (`ddgs.results.ImagesResult`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageResult {
    /// Image title (`null` when the engine's data lacks it).
    pub title: Option<String>,
    /// Full image URL.
    pub image: Option<String>,
    /// Thumbnail URL.
    pub thumbnail: Option<String>,
    /// Page URL containing the image.
    pub url: Option<String>,
    /// Image height as the engine gives it: a JSON integer (DuckDuckGo), a
    /// string (Bing), `""` when unknown, or `null` when missing.
    pub height: Value,
    /// Image width, typed like [`ImageResult::height`].
    pub width: Value,
    /// Source label reported by the engine (e.g. `"Bing"`, a domain).
    pub source: Option<String>,
}

impl Default for ImageResult {
    fn default() -> Self {
        Self {
            title: empty(),
            image: empty(),
            thumbnail: empty(),
            url: empty(),
            height: Value::String(String::new()),
            width: Value::String(String::new()),
            source: empty(),
        }
    }
}

impl ImageResult {
    /// Apply upstream field normalization to every mapped field.
    ///
    /// Not idempotent: use it once, on raw values (see the module docs).
    pub fn normalized(mut self) -> Self {
        text_opt(&mut self.title);
        url_opt(&mut self.image);
        url_opt(&mut self.thumbnail);
        url_opt(&mut self.url);
        self
    }
}

/// A news-search result (`ddgs.results.NewsResult`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewsResult {
    /// Publication date: ISO-8601 when the engine provides a timestamp or a
    /// parseable date, otherwise the engine's original date text.
    pub date: Option<String>,
    /// Result title.
    pub title: Option<String>,
    /// Snippet/body text.
    pub body: Option<String>,
    /// Article URL.
    pub url: Option<String>,
    /// Article image URL.
    pub image: Option<String>,
    /// News publisher/source name.
    pub source: Option<String>,
}

impl Default for NewsResult {
    fn default() -> Self {
        Self {
            date: empty(),
            title: empty(),
            body: empty(),
            url: empty(),
            image: empty(),
            source: empty(),
        }
    }
}

impl NewsResult {
    /// Construct a news result from raw string values (every field present),
    /// applying upstream field normalization.
    pub fn new(
        date: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
        url: impl Into<String>,
        image: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            date: Some(date.into()),
            title: Some(title.into()),
            body: Some(body.into()),
            url: Some(url.into()),
            image: Some(image.into()),
            source: Some(source.into()),
        }
        .normalized()
    }

    /// Apply upstream field normalization to every mapped field.
    ///
    /// Not idempotent: use it once, on raw values (see the module docs).
    pub fn normalized(mut self) -> Self {
        text_opt(&mut self.title);
        text_opt(&mut self.body);
        url_opt(&mut self.url);
        url_opt(&mut self.image);
        self
    }
}

/// A video-search result (`ddgs.results.VideosResult`).
///
/// Build from [`Default`] with struct-update syntax, then call
/// [`VideoResult::normalized`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoResult {
    /// Video title.
    pub title: Option<String>,
    /// Video page URL.
    pub content: Option<String>,
    /// Video description (stored verbatim, as upstream).
    pub description: Option<String>,
    /// Duration text, e.g. `"12:34"`.
    pub duration: Option<String>,
    /// Embed HTML snippet.
    pub embed_html: Option<String>,
    /// Embed URL.
    pub embed_url: Option<String>,
    /// Opaque image token.
    pub image_token: Option<String>,
    /// Thumbnail URLs keyed by size (`large`, `medium`, `motion`, `small`),
    /// copied verbatim from the engine's JSON (an object, or `null`).
    pub images: Value,
    /// Upstream data provider (e.g. `"Bing"`).
    pub provider: Option<String>,
    /// Published date text.
    pub published: Option<String>,
    /// Publisher/platform name.
    pub publisher: Option<String>,
    /// Statistics (e.g. `viewCount`), copied verbatim from the engine's JSON,
    /// so values keep their JSON types (integers, `null`).
    pub statistics: Value,
    /// Uploader/channel name.
    pub uploader: Option<String>,
}

impl Default for VideoResult {
    fn default() -> Self {
        Self {
            title: empty(),
            content: empty(),
            description: empty(),
            duration: empty(),
            embed_html: empty(),
            embed_url: empty(),
            image_token: empty(),
            images: Value::Object(serde_json::Map::new()),
            provider: empty(),
            published: empty(),
            publisher: empty(),
            statistics: Value::Object(serde_json::Map::new()),
            uploader: empty(),
        }
    }
}

impl VideoResult {
    /// Apply upstream field normalization to every mapped field (only `title`
    /// and `publisher` are in upstream's map for videos).
    ///
    /// Not idempotent: use it once, on raw values (see the module docs).
    pub fn normalized(mut self) -> Self {
        text_opt(&mut self.title);
        text_opt(&mut self.publisher);
        self
    }
}

/// A book-search result (`ddgs.results.BooksResult`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BooksResult {
    /// Book title.
    pub title: String,
    /// Author name(s).
    pub author: String,
    /// Publisher name.
    pub publisher: String,
    /// Additional metadata/format information.
    pub info: String,
    /// Book detail page URL.
    pub url: String,
    /// Cover thumbnail URL.
    pub thumbnail: String,
}

impl BooksResult {
    /// Construct a book result from raw values, applying upstream field
    /// normalization.
    pub fn new(
        title: impl Into<String>,
        author: impl Into<String>,
        publisher: impl Into<String>,
        info: impl Into<String>,
        url: impl Into<String>,
        thumbnail: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            author: author.into(),
            publisher: publisher.into(),
            info: info.into(),
            url: url.into(),
            thumbnail: thumbnail.into(),
        }
        .normalized()
    }

    /// Apply upstream field normalization to every mapped field.
    ///
    /// Not idempotent: use it once, on raw values (see the module docs).
    pub fn normalized(mut self) -> Self {
        text(&mut self.title);
        text(&mut self.author);
        text(&mut self.publisher);
        text(&mut self.info);
        url(&mut self.url);
        url(&mut self.thumbnail);
        self
    }
}

/// A search result across supported search kinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchResult {
    /// Text/web result.
    Text(TextResult),
    /// Image result.
    Image(ImageResult),
    /// Video result.
    Video(VideoResult),
    /// News result.
    News(NewsResult),
    /// Books result.
    Books(BooksResult),
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exact-string assertions pin both the field names and the JSON key
    // order, which must equal the upstream dataclass field order.

    #[test]
    fn text_json_shape() {
        let result = TextResult::new("t", "https://x.com", "b");
        assert_eq!(
            serde_json::to_string(&result).unwrap(),
            r#"{"title":"t","href":"https://x.com","body":"b"}"#
        );
    }

    #[test]
    fn images_json_shape() {
        assert_eq!(
            serde_json::to_string(&ImageResult::default()).unwrap(),
            r#"{"title":"","image":"","thumbnail":"","url":"","height":"","width":"","source":""}"#
        );
        // DuckDuckGo shape: integer dimensions, a missing key as null.
        let ddg = ImageResult {
            title: None,
            height: Value::from(444),
            width: Value::from(700),
            ..ImageResult::default()
        };
        assert_eq!(
            serde_json::to_string(&ddg).unwrap(),
            r#"{"title":null,"image":"","thumbnail":"","url":"","height":444,"width":700,"source":""}"#
        );
    }

    #[test]
    fn news_json_shape() {
        assert_eq!(
            serde_json::to_string(&NewsResult::default()).unwrap(),
            r#"{"date":"","title":"","body":"","url":"","image":"","source":""}"#
        );
        let missing_image = NewsResult {
            image: None,
            ..NewsResult::default()
        };
        assert_eq!(
            serde_json::to_string(&missing_image).unwrap(),
            r#"{"date":"","title":"","body":"","url":"","image":null,"source":""}"#
        );
    }

    #[test]
    fn videos_json_shape() {
        assert_eq!(
            serde_json::to_string(&VideoResult::default()).unwrap(),
            r#"{"title":"","content":"","description":"","duration":"","embed_html":"","embed_url":"","image_token":"","images":{},"provider":"","published":"","publisher":"","statistics":{},"uploader":""}"#
        );
        let raw = VideoResult {
            image_token: None,
            statistics: serde_json::json!({"viewCount": 12, "likeCount": null}),
            ..VideoResult::default()
        };
        assert_eq!(
            serde_json::to_string(&raw).unwrap(),
            r#"{"title":"","content":"","description":"","duration":"","embed_html":"","embed_url":"","image_token":null,"images":{},"provider":"","published":"","publisher":"","statistics":{"viewCount":12,"likeCount":null},"uploader":""}"#
        );
    }

    #[test]
    fn books_json_shape() {
        assert_eq!(
            serde_json::to_string(&BooksResult::default()).unwrap(),
            r#"{"title":"","author":"","publisher":"","info":"","url":"","thumbnail":""}"#
        );
    }

    #[test]
    fn constructors_apply_field_map() {
        let text_result = TextResult::new(" <b>A</b>&amp;B ", "https://x.com/a%20b", "x\ty");
        assert_eq!(text_result.title, "A&B");
        assert_eq!(text_result.href, "https://x.com/a+b");
        assert_eq!(text_result.body, "xy");

        let news = NewsResult::new("3 days", "<i>T</i>", "b", "u%2F", "i%20j", " S ");
        assert_eq!(news.date.as_deref(), Some("3 days"));
        assert_eq!(news.title.as_deref(), Some("T"));
        assert_eq!(news.url.as_deref(), Some("u/"));
        assert_eq!(news.image.as_deref(), Some("i+j"));
        // `source` is not in upstream's normalizer map.
        assert_eq!(news.source.as_deref(), Some(" S "));

        let video = VideoResult {
            title: Some("<b>V</b>".into()),
            description: Some("<b>raw</b>".into()),
            publisher: Some(" You  Tube ".into()),
            content: Some("https://x.com/a%20b".into()),
            uploader: None,
            ..VideoResult::default()
        }
        .normalized();
        assert_eq!(video.title.as_deref(), Some("V"));
        assert_eq!(video.publisher.as_deref(), Some("You Tube"));
        assert_eq!(video.description.as_deref(), Some("<b>raw</b>"));
        assert_eq!(video.content.as_deref(), Some("https://x.com/a%20b"));
        assert_eq!(video.uploader, None);
    }

    #[test]
    fn normalized_is_not_idempotent() {
        let once = TextResult::new("a &amp;lt;b&amp;gt; c", "https://x.com/%2520", "");
        assert_eq!(once.title, "a &lt;b&gt; c");
        assert_eq!(once.href, "https://x.com/%20");
        let twice = once.normalized();
        assert_eq!(twice.title, "a <b> c");
        assert_eq!(twice.href, "https://x.com/+");
    }

    #[test]
    fn search_result_enum_round_trips() {
        let value = SearchResult::News(NewsResult::new("d", "t", "b", "u", "", "s"));
        let json = serde_json::to_string(&value).unwrap();
        assert!(json.starts_with(r#"{"kind":"news","date":"d""#));
        let back: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);
    }
}
