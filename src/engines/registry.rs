//! The engine registry: which engines serve each category.
//!
//! Grows as engines are ported. News, images, videos, and books sets are added
//! alongside `text` as their engines land; Google is intentionally absent until
//! Python `ddgs` re-enables it.

use std::sync::Arc;

use crate::engines::{EngineSet, books, images, news, text, videos};
use crate::{
    BooksResult, BooksSearchRequest, ImageResult, ImageSearchRequest, NewsResult,
    NewsSearchRequest, TextResult, TextSearchRequest, VideoResult, VideoSearchRequest,
};

/// All engines, grouped by category.
pub struct EngineRegistry {
    text: EngineSet<TextSearchRequest, TextResult>,
    news: EngineSet<NewsSearchRequest, NewsResult>,
    images: EngineSet<ImageSearchRequest, ImageResult>,
    videos: EngineSet<VideoSearchRequest, VideoResult>,
    books: EngineSet<BooksSearchRequest, BooksResult>,
}

impl EngineRegistry {
    /// Build the registry with every available engine registered.
    pub fn new() -> Self {
        Self {
            text: EngineSet::new(vec![
                Arc::new(text::Wikipedia::new()),
                Arc::new(text::DuckDuckGo::new()),
                Arc::new(text::Bing::new()),
                Arc::new(text::Brave::new()),
                Arc::new(text::Grokipedia::new()),
                Arc::new(text::Mojeek::new()),
                Arc::new(text::Yahoo::new()),
                Arc::new(text::Startpage::new()),
                Arc::new(text::Yandex::new()),
            ]),
            news: EngineSet::new(vec![
                Arc::new(news::DuckDuckGoNews::new()),
                Arc::new(news::BingNews::new()),
                Arc::new(news::YahooNews::new()),
            ]),
            images: EngineSet::new(vec![
                Arc::new(images::DuckDuckGoImages::new()),
                Arc::new(images::BingImages::new()),
            ]),
            videos: EngineSet::new(vec![Arc::new(videos::DuckDuckGoVideos::new())]),
            books: EngineSet::new(vec![Arc::new(books::AnnasArchive::new())]),
        }
    }

    /// Engines serving text search.
    pub fn text(&self) -> &EngineSet<TextSearchRequest, TextResult> {
        &self.text
    }

    /// Engines serving news search.
    pub fn news(&self) -> &EngineSet<NewsSearchRequest, NewsResult> {
        &self.news
    }

    /// Engines serving image search.
    pub fn images(&self) -> &EngineSet<ImageSearchRequest, ImageResult> {
        &self.images
    }

    /// Engines serving video search.
    pub fn videos(&self) -> &EngineSet<VideoSearchRequest, VideoResult> {
        &self.videos
    }

    /// Engines serving books search.
    pub fn books(&self) -> &EngineSet<BooksSearchRequest, BooksResult> {
        &self.books
    }
}

impl Default for EngineRegistry {
    fn default() -> Self {
        Self::new()
    }
}
