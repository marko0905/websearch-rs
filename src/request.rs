use std::{fmt, str::FromStr};

use crate::{Result, WebSearchError};

/// DuckDuckGo region code, for example `wt-wt`, `us-en`, or `de-de`.
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
        Self("wt-wt".to_owned())
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

/// Time filter for search results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeLimit {
    Day,
    Week,
    Month,
    Year,
}

impl TimeLimit {
    pub(crate) fn as_duckduckgo_df(self) -> &'static str {
        match self {
            Self::Day => "d",
            Self::Week => "w",
            Self::Month => "m",
            Self::Year => "y",
        }
    }
}

impl fmt::Display for TimeLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        })
    }
}

impl FromStr for TimeLimit {
    type Err = WebSearchError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "d" | "day" | "days" => Ok(Self::Day),
            "w" | "week" | "weeks" => Ok(Self::Week),
            "m" | "month" | "months" => Ok(Self::Month),
            "y" | "year" | "years" => Ok(Self::Year),
            other => Err(WebSearchError::InvalidConfig(format!(
                "unknown time-limit value `{other}`"
            ))),
        }
    }
}

/// Shared validation for paged search requests.
fn validate_paged(query: &str, max_results: usize, page: usize) -> Result<()> {
    if query.trim().is_empty() {
        return Err(WebSearchError::InvalidConfig(
            "query cannot be empty".into(),
        ));
    }
    if max_results == 0 {
        return Err(WebSearchError::InvalidConfig(
            "max_results must be greater than zero".into(),
        ));
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
    /// Maximum number of results to return.
    pub max_results: usize,
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
            max_results: 10,
            region: Region::default(),
            safe_search: SafeSearch::default(),
            time_limit: None,
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count.
    pub fn max_results(mut self, max_results: usize) -> Self {
        self.max_results = max_results;
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
        validate_paged(&self.query, self.max_results, self.page)
    }
}

/// Request for video search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return.
    pub max_results: usize,
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

impl VideoSearchRequest {
    /// Create a video search request with defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: 10,
            region: Region::default(),
            safe_search: SafeSearch::default(),
            time_limit: None,
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count.
    pub fn max_results(mut self, max_results: usize) -> Self {
        self.max_results = max_results;
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
        validate_paged(&self.query, self.max_results, self.page)
    }
}

/// Request for image search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return.
    pub max_results: usize,
    /// Provider region code.
    pub region: Region,
    /// Safe-search mode.
    pub safe_search: SafeSearch,
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
            max_results: 10,
            region: Region::default(),
            safe_search: SafeSearch::default(),
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count.
    pub fn max_results(mut self, max_results: usize) -> Self {
        self.max_results = max_results;
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
        validate_paged(&self.query, self.max_results, self.page)
    }
}

/// Request for news search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewsSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return.
    pub max_results: usize,
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
            max_results: 10,
            region: Region::default(),
            safe_search: SafeSearch::default(),
            time_limit: None,
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count.
    pub fn max_results(mut self, max_results: usize) -> Self {
        self.max_results = max_results;
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
        validate_paged(&self.query, self.max_results, self.page)
    }
}

/// Request for books search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BooksSearchRequest {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return.
    pub max_results: usize,
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
            max_results: 10,
            backend: "auto".to_owned(),
            page: 1,
        }
    }

    /// Set maximum result count.
    pub fn max_results(mut self, max_results: usize) -> Self {
        self.max_results = max_results;
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
        validate_paged(&self.query, self.max_results, self.page)
    }
}
