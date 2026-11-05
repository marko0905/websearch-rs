//! Rust implementation of the `ddgs` metasearch library.
//!
//! Aggregates results from multiple search engines behind one async client
//! ([`WebSearchClient`]), a blocking wrapper ([`WebSearchBlockingClient`]), and a CLI. It
//! covers five categories — text, news, images, videos, and books — running each
//! category's engines concurrently, then deduplicating and ranking the results.

pub mod blocking;
#[doc(hidden)]
pub mod cli;
pub mod client;
pub mod config;
pub mod engines;
pub mod error;
pub mod extract;
pub(crate) mod html;
pub mod http;
pub(crate) mod normalize;
pub mod ranking;
pub mod request;
pub mod results;
pub(crate) mod server;

pub use blocking::WebSearchBlockingClient;
pub use client::{WebSearchClient, WebSearchClientBuilder};
pub use config::WebSearchConfig;
pub use error::{Result, WebSearchError};
pub use extract::{ExtractContent, ExtractFormat, ExtractResult};
pub use http::Verify;
pub use request::{
    BooksSearchRequest, ImageSearchRequest, NewsSearchRequest, Region, SafeSearch,
    TextSearchRequest, TimeLimit, VideoSearchRequest,
};
pub use results::{BooksResult, ImageResult, NewsResult, SearchResult, TextResult, VideoResult};
