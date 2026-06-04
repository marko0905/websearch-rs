//! Text-category engines.

pub mod bing;
pub mod brave;
pub mod duckduckgo;
pub mod grokipedia;
pub mod mojeek;
pub mod startpage;
pub mod wikipedia;
pub mod yahoo;
pub mod yandex;

pub use bing::Bing;
pub use brave::Brave;
pub use duckduckgo::DuckDuckGo;
pub use grokipedia::Grokipedia;
pub use mojeek::Mojeek;
pub use startpage::Startpage;
pub use wikipedia::Wikipedia;
pub use yahoo::Yahoo;
pub use yandex::Yandex;
