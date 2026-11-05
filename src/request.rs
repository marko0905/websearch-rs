use std::{fmt, str::FromStr};

use crate::{Result, WebSearchError};

/// Region code, for example `us-en`, `uk-en`, or `de-de` (default `us-en`, as
/// upstream `ddgs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region(String);

impl Region {
    /// Create a region from a provider region code.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(WebSearchError::InvalidConfig(
                "region cannot be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    /// Return the region code as accepted by DuckDuckGo's `kl` parameter.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Region {
    fn default() -> Self {
        // Upstream `region: str = "us-en"` (ddgs.py `_search_sync`, cli.py).
        Self("us-en".to_owned())
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Region {
    type Err = WebSearchError;

    fn from_str(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

/// Safe-search setting.
///
/// Values match Python `ddgs`: `on` / `moderate` / `off`. (`strict` is accepted
/// as a legacy alias for `on` when parsing.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SafeSearch {
    /// Disable safe search.
    Off,
    /// Moderate filtering.
    #[default]
    Moderate,
    /// Maximum filtering.
    On,
}

impl SafeSearch {
    pub(crate) fn as_duckduckgo_kp(self) -> &'static str {
        match self {
            Self::Off => "-2",
            Self::Moderate => "-1",
            Self::On => "1",
        }
    }
}

impl fmt::Display for SafeSearch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Moderate => "moderate",
            Self::On => "on",
        })
    }
}

impl FromStr for SafeSearch {
    type Err = WebSearchError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "false" | "0" => Ok(Self::Off),
            "moderate" | "medium" | "normal" => Ok(Self::Moderate),
            "on" | "strict" | "true" | "1" => Ok(Self::On),
            other => Err(WebSearchError::InvalidConfig(format!(
                "unknown safe-search value `{other}`"
            ))),
        }
    }
}

/// Time filter for search results (upstream `timelimit: str | None`).
///
/// Upstream passes the string through to each engine: engines that send it
/// raw (DuckDuckGo text/news/videos, Yahoo text/news, Startpage) accept any
/// value, such as a custom date range; engines that look it up in a table
/// (Brave, Bing text/news/images, DuckDuckGo images) raise `KeyError` for
/// anything but `d`/`w`/`m`/`y`, so that engine fails. [`TimeLimit::Custom`]
/// carries such a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeLimit {
    /// `"d"`.
    Day,
    /// `"w"`.
    Week,
    /// `"m"`.
    Month,
    /// `"y"`.
    Year,
    /// Any other value, passed through as is.
    Custom(String),
}

impl TimeLimit {
    /// The upstream string value.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Day => "d",
            Self::Week => "w",
            Self::Month => "m",
            Self::Year => "y",
            Self::Custom(value) => value,
        }
    }

    /// The value engines send raw (`df`, `btf`, `with_date`, ...).
    pub(crate) fn as_duckduckgo_df(&self) -> &str {
        self.as_str()
    }

    /// Python's `KeyError` for engines that map the value through a table.
    pub(crate) fn key_error(&self) -> WebSearchError {
        WebSearchError::Internal(crate::engines::util::py_repr(self.as_str()))
    }
}

impl fmt::Display for TimeLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TimeLimit {
    type Err = WebSearchError;

    /// Exact upstream values: `d`, `w`, `m`, `y`; any other non-empty string
    /// is [`TimeLimit::Custom`]. An empty string is falsy upstream (no time
    /// limit), so it is an error here; callers map it to `None`.
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "d" => Ok(Self::Day),
            "w" => Ok(Self::Week),
            "m" => Ok(Self::Month),
            "y" => Ok(Self::Year),
            "" => Err(WebSearchError::InvalidConfig(
                "time limit cannot be empty".into(),
            )),
            other => Ok(Self::Custom(other.to_owned())),
        }
    }
}

/// Shared validation for paged search requests.
///
/// Upstream `_search_sync` checks only `if not query: raise
/// DDGSException("query is mandatory.")` — an empty string fails, a
/// whitespace-only query is searched as is. `max_results` is not validated
/// (`None`/`0` mean "all results"). Upstream does not check `page` either;
/// Rust keeps `page >= 1` because the engines' offset arithmetic
/// (`page - 1`) cannot go negative on `usize`.
fn validate_paged(query: &str, page: usize) -> Result<()> {
    if query.is_empty() {
        return Err(WebSearchError::InvalidConfig("query is mandatory.".into()));
    }
    if page == 0 {
        return Err(WebSearchError::InvalidConfig(
            "page must be 1 or greater".into(),
        ));
    }
    Ok(())
}

/// Request for text search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return; `None` (or `Some(0)`) returns
    /// every aggregated result, as upstream `max_results=None`/`0`.
    pub max_results: Option<usize>,
    /// Provider region code.
    pub region: Region,
    /// Safe-search mode.
    pub safe_search: SafeSearch,
    /// Optional time filter.
    pub time_limit: Option<TimeLimit>,
    /// Engine selection: `auto` (all engines) or a comma-separated engine list.
    pub backend: String,
    /// 1-based result page.
    pub page: usize,
}

impl TextSearchRequest {
    /// Create a text search request with defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: Some(10),
            region: Region::default(),
            safe_search: SafeSearch::default(),
            time_limit: None,
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count (`None` or `0`: all results).
    pub fn max_results(mut self, max_results: impl Into<Option<usize>>) -> Self {
        self.max_results = max_results.into();
        self
    }

    /// Set provider region.
    pub fn region(mut self, region: Region) -> Self {
        self.region = region;
        self
    }

    /// Set safe-search mode.
    pub fn safe_search(mut self, safe_search: SafeSearch) -> Self {
        self.safe_search = safe_search;
        self
    }

    /// Set time filter.
    pub fn time_limit(mut self, time_limit: Option<TimeLimit>) -> Self {
        self.time_limit = time_limit;
        self
    }

    /// Set engine backend (`auto` or a comma-separated engine list).
    pub fn backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = backend.into();
        self
    }

    /// Set the 1-based result page.
    pub fn page(mut self, page: usize) -> Self {
        self.page = page;
        self
    }

    /// Validate the request before sending it to a provider.
    pub fn validate(&self) -> Result<()> {
        validate_paged(&self.query, self.page)
    }
}

/// Request for video search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return; `None` (or `Some(0)`) returns
    /// every aggregated result, as upstream `max_results=None`/`0`.
    pub max_results: Option<usize>,
    /// Provider region code.
    pub region: Region,
    /// Safe-search mode.
    pub safe_search: SafeSearch,
    /// Optional time filter.
    pub time_limit: Option<TimeLimit>,
    /// Resolution filter, e.g. `high`, `standart` (sic, upstream spelling).
    pub resolution: Option<String>,
    /// Duration filter, e.g. `short`, `medium`, `long`.
    pub duration: Option<String>,
    /// License filter, e.g. `creativeCommon`, `youtube`.
    pub license_videos: Option<String>,
    /// Engine selection: `auto` (all engines) or a comma-separated engine list.
    pub backend: String,
    /// 1-based result page.
    pub page: usize,
}

impl VideoSearchRequest {
    /// Create a video search request with defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: Some(10),
            region: Region::default(),
            safe_search: SafeSearch::default(),
            time_limit: None,
            resolution: None,
            duration: None,
            license_videos: None,
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count (`None` or `0`: all results).
    pub fn max_results(mut self, max_results: impl Into<Option<usize>>) -> Self {
        self.max_results = max_results.into();
        self
    }

    /// Set provider region.
    pub fn region(mut self, region: Region) -> Self {
        self.region = region;
        self
    }

    /// Set safe-search mode.
    pub fn safe_search(mut self, safe_search: SafeSearch) -> Self {
        self.safe_search = safe_search;
        self
    }

    /// Set time filter.
    pub fn time_limit(mut self, time_limit: Option<TimeLimit>) -> Self {
        self.time_limit = time_limit;
        self
    }

    /// Set the `resolution` filter (upstream `resolution` keyword; passed as is, not
    /// validated — an empty string counts as unset).
    pub fn resolution(mut self, resolution: impl Into<String>) -> Self {
        self.resolution = Some(resolution.into());
        self
    }

    /// Set the `duration` filter (upstream `duration` keyword; passed as is, not
    /// validated — an empty string counts as unset).
    pub fn duration(mut self, duration: impl Into<String>) -> Self {
        self.duration = Some(duration.into());
        self
    }

    /// Set the `license_videos` filter (upstream `license_videos` keyword; passed as is, not
    /// validated — an empty string counts as unset).
    pub fn license_videos(mut self, license_videos: impl Into<String>) -> Self {
        self.license_videos = Some(license_videos.into());
        self
    }

    /// Set engine backend (`auto` or a comma-separated engine list).
    pub fn backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = backend.into();
        self
    }

    /// Set the 1-based result page.
    pub fn page(mut self, page: usize) -> Self {
        self.page = page;
        self
    }

    /// Validate the request before sending it to a provider.
    pub fn validate(&self) -> Result<()> {
        validate_paged(&self.query, self.page)
    }
}

/// Request for image search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return; `None` (or `Some(0)`) returns
    /// every aggregated result, as upstream `max_results=None`/`0`.
    pub max_results: Option<usize>,
    /// Provider region code.
    pub region: Region,
    /// Safe-search mode.
    pub safe_search: SafeSearch,
    /// Optional time filter.
    pub time_limit: Option<TimeLimit>,
    /// Image size filter, e.g. `Small`, `Medium`, `Large`, `Wallpaper`.
    pub size: Option<String>,
    /// Color filter, e.g. `color`, `Monochrome`, `Red`, …
    pub color: Option<String>,
    /// Image type filter, e.g. `photo`, `clipart`, `gif`, `transparent`, `line`.
    pub type_image: Option<String>,
    /// Layout filter, e.g. `Square`, `Tall`, `Wide`.
    pub layout: Option<String>,
    /// License filter, e.g. `any`, `Public`, `Share`, `ShareCommercially`, `Modify`, `ModifyCommercially`.
    pub license_image: Option<String>,
    /// Engine selection: `auto` (all engines) or a comma-separated engine list.
    pub backend: String,
    /// 1-based result page.
    pub page: usize,
}

impl ImageSearchRequest {
    /// Create an image search request with defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: Some(10),
            region: Region::default(),
            safe_search: SafeSearch::default(),
            time_limit: None,
            size: None,
            color: None,
            type_image: None,
            layout: None,
            license_image: None,
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count (`None` or `0`: all results).
    pub fn max_results(mut self, max_results: impl Into<Option<usize>>) -> Self {
        self.max_results = max_results.into();
        self
    }

    /// Set provider region.
    pub fn region(mut self, region: Region) -> Self {
        self.region = region;
        self
    }

    /// Set safe-search mode.
    pub fn safe_search(mut self, safe_search: SafeSearch) -> Self {
        self.safe_search = safe_search;
        self
    }

    /// Set time filter.
    pub fn time_limit(mut self, time_limit: Option<TimeLimit>) -> Self {
        self.time_limit = time_limit;
        self
    }

    /// Set the `size` filter (upstream `size` keyword; passed as is, not
    /// validated — an empty string counts as unset).
    pub fn size(mut self, size: impl Into<String>) -> Self {
        self.size = Some(size.into());
        self
    }

    /// Set the `color` filter (upstream `color` keyword; passed as is, not
    /// validated — an empty string counts as unset).
    pub fn color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Set the `type_image` filter (upstream `type_image` keyword; passed as is, not
    /// validated — an empty string counts as unset).
    pub fn type_image(mut self, type_image: impl Into<String>) -> Self {
        self.type_image = Some(type_image.into());
        self
    }

    /// Set the `layout` filter (upstream `layout` keyword; passed as is, not
    /// validated — an empty string counts as unset).
    pub fn layout(mut self, layout: impl Into<String>) -> Self {
        self.layout = Some(layout.into());
        self
    }

    /// Set the `license_image` filter (upstream `license_image` keyword; passed as is, not
    /// validated — an empty string counts as unset).
    pub fn license_image(mut self, license_image: impl Into<String>) -> Self {
        self.license_image = Some(license_image.into());
        self
    }

    /// Set engine backend (`auto` or a comma-separated engine list).
    pub fn backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = backend.into();
        self
    }

    /// Set the 1-based result page.
    pub fn page(mut self, page: usize) -> Self {
        self.page = page;
        self
    }

    /// Validate the request before sending it to a provider.
    pub fn validate(&self) -> Result<()> {
        validate_paged(&self.query, self.page)
    }
}

/// Request for news search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewsSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return; `None` (or `Some(0)`) returns
    /// every aggregated result, as upstream `max_results=None`/`0`.
    pub max_results: Option<usize>,
    /// Provider region code.
    pub region: Region,
    /// Safe-search mode.
    pub safe_search: SafeSearch,
    /// Optional time filter.
    pub time_limit: Option<TimeLimit>,
    /// Engine selection: `auto` (all engines) or a comma-separated engine list.
    pub backend: String,
    /// 1-based result page.
    pub page: usize,
}

impl NewsSearchRequest {
    /// Create a news search request with defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: Some(10),
            region: Region::default(),
            safe_search: SafeSearch::default(),
            time_limit: None,
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count (`None` or `0`: all results).
    pub fn max_results(mut self, max_results: impl Into<Option<usize>>) -> Self {
        self.max_results = max_results.into();
        self
    }

    /// Set provider region.
    pub fn region(mut self, region: Region) -> Self {
        self.region = region;
        self
    }

    /// Set safe-search mode.
    pub fn safe_search(mut self, safe_search: SafeSearch) -> Self {
        self.safe_search = safe_search;
        self
    }

    /// Set time filter.
    pub fn time_limit(mut self, time_limit: Option<TimeLimit>) -> Self {
        self.time_limit = time_limit;
        self
    }

    /// Set engine backend (`auto` or a comma-separated engine list).
    pub fn backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = backend.into();
        self
    }

    /// Set the 1-based result page.
    pub fn page(mut self, page: usize) -> Self {
        self.page = page;
        self
    }

    /// Validate the request before sending it to a provider.
    pub fn validate(&self) -> Result<()> {
        validate_paged(&self.query, self.page)
    }
}

/// Request for books search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BooksSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return; `None` (or `Some(0)`) returns
    /// every aggregated result, as upstream `max_results=None`/`0`.
    pub max_results: Option<usize>,
    /// Engine selection: `auto` (all engines) or a comma-separated engine list.
    pub backend: String,
    /// 1-based result page.
    pub page: usize,
}

impl BooksSearchRequest {
    /// Create a books search request with defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: Some(10),
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count (`None` or `0`: all results).
    pub fn max_results(mut self, max_results: impl Into<Option<usize>>) -> Self {
        self.max_results = max_results.into();
        self
    }

    /// Set engine backend (`auto` or a comma-separated engine list).
    pub fn backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = backend.into();
        self
    }

    /// Set the 1-based result page.
    pub fn page(mut self, page: usize) -> Self {
        self.page = page;
        self
    }

    /// Validate the request before sending it to a provider.
    pub fn validate(&self) -> Result<()> {
        validate_paged(&self.query, self.page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_limit_parses_like_upstream() {
        assert_eq!("d".parse::<TimeLimit>().unwrap(), TimeLimit::Day);
        assert_eq!("y".parse::<TimeLimit>().unwrap(), TimeLimit::Year);
        // Upstream has no aliases: anything else is passed through as is.
        assert_eq!(
            "day".parse::<TimeLimit>().unwrap(),
            TimeLimit::Custom("day".into())
        );
        assert_eq!(
            "2024-01-01..2024-12-31"
                .parse::<TimeLimit>()
                .unwrap()
                .as_str(),
            "2024-01-01..2024-12-31"
        );
        assert!("".parse::<TimeLimit>().is_err());
        assert_eq!(TimeLimit::Week.to_string(), "w");
    }
}
