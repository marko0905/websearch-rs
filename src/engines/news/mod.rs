//! News-category engines.

pub mod bing;
pub mod duckduckgo;
pub mod yahoo;

pub use bing::BingNews;
pub use duckduckgo::DuckDuckGoNews;
pub use yahoo::YahooNews;
