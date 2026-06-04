use serde::{Deserialize, Serialize};

/// A normalized text-search result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextResult {
    /// Result title.
    pub title: String,
    /// Destination URL.
    pub href: String,
    /// Snippet/body text shown by the provider.
    pub body: String,
    /// Provider that produced the result.
    pub source: String,
}

/// A normalized video-search result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoResult {
    /// Video title.
    pub title: String,
    /// Video page/content URL.
    pub content: String,
    /// Video description/snippet.
    pub description: String,
    /// Thumbnail URL when available.
    pub thumbnail: String,
    /// Embed URL when available.
    pub embed_url: Option<String>,
    /// Duration text when available.
    pub duration: Option<String>,
    /// Published date string when available.
    pub published: Option<String>,
    /// Publisher/platform name.
    pub publisher: String,
    /// Uploader/channel name.
    pub uploader: String,
    /// Upstream source label.
    pub source: String,
    /// Provider that produced the result.
    pub provider: String,
}

impl VideoResult {
    /// Construct a video result.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: impl Into<String>,
        content: impl Into<String>,
        description: impl Into<String>,
        thumbnail: impl Into<String>,
        embed_url: Option<String>,
        duration: Option<String>,
        published: Option<String>,
        publisher: impl Into<String>,
        uploader: impl Into<String>,
        source: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
            description: description.into(),
            thumbnail: thumbnail.into(),
            embed_url,
            duration,
            published,
            publisher: publisher.into(),
            uploader: uploader.into(),
            source: source.into(),
            provider: provider.into(),
        }
    }
}

/// A normalized image-search result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageResult {
    /// Image title.
    pub title: String,
    /// Full image URL.
    pub image: String,
    /// Thumbnail URL.
    pub thumbnail: String,
    /// Page URL containing the image.
    pub url: String,
    /// Image width in pixels when available.
    pub width: Option<u64>,
    /// Image height in pixels when available.
    pub height: Option<u64>,
    /// Upstream source label.
    pub source: String,
    /// Provider that produced the result.
    pub provider: String,
}

impl ImageResult {
    /// Construct an image result.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: impl Into<String>,
        image: impl Into<String>,
        thumbnail: impl Into<String>,
        url: impl Into<String>,
        width: Option<u64>,
        height: Option<u64>,
        source: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            image: image.into(),
            thumbnail: thumbnail.into(),
            url: url.into(),
            width,
            height,
            source: source.into(),
            provider: provider.into(),
        }
    }
}

/// A normalized news-search result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsResult {
    /// Result title.
    pub title: String,
    /// Destination URL.
    pub href: String,
    /// Snippet/body text shown by the provider.
    pub body: String,
    /// Unix timestamp published date when available.
    pub date: Option<u64>,
    /// News publisher/source name.
    pub source: String,
    /// Provider that produced the result.
    pub provider: String,
}

impl NewsResult {
    /// Construct a news result.
    pub fn new(
        title: impl Into<String>,
        href: impl Into<String>,
        body: impl Into<String>,
        date: Option<u64>,
        source: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            href: href.into(),
            body: body.into(),
            date,
            source: source.into(),
            provider: provider.into(),
        }
    }
}

/// A normalized book-search result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BooksResult {
    /// Book title.
    pub title: String,
    /// Author name(s) when available.
    pub author: String,
    /// Publisher name when available.
    pub publisher: String,
    /// Additional metadata/format information.
    pub info: String,
    /// Book detail page URL.
    pub url: String,
    /// Cover thumbnail URL when available.
    pub thumbnail: String,
    /// Provider that produced the result.
    pub provider: String,
}

impl BooksResult {
    /// Construct a book result.
    pub fn new(
        title: impl Into<String>,
        author: impl Into<String>,
        publisher: impl Into<String>,
        info: impl Into<String>,
        url: impl Into<String>,
        thumbnail: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            author: author.into(),
            publisher: publisher.into(),
            info: info.into(),
            url: url.into(),
            thumbnail: thumbnail.into(),
            provider: provider.into(),
        }
    }
}

/// A normalized search result across supported search kinds.
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

impl TextResult {
    /// Construct a text result.
    pub fn new(
        title: impl Into<String>,
        href: impl Into<String>,
        body: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            href: href.into(),
            body: body.into(),
            source: source.into(),
        }
    }
}
