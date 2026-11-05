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

#[allow(unused_imports)] // disabled upstream (ddgs v9.16.1); kept for re-enabling
pub use bing::Bing;
pub use brave::Brave;
pub use duckduckgo::DuckDuckGo;
pub use grokipedia::Grokipedia;
pub use mojeek::Mojeek;
pub use startpage::Startpage;
pub use wikipedia::Wikipedia;
pub use yahoo::Yahoo;
#[allow(unused_imports)] // disabled upstream (ddgs v9.16.1); kept for re-enabling
pub use yandex::Yandex;
