use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use websearch_rs::{
    BooksSearchRequest, ImageSearchRequest, NewsSearchRequest, Region, SafeSearch,
    TextSearchRequest, TimeLimit, VideoSearchRequest, WebSearchClient,
};

#[derive(Debug, Parser)]
#[command(name = "websearch")]
#[command(about = "Search from the command line with websearch-rs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Debug, Args)]
struct ClientArgs {
    /// Request timeout in seconds.
    #[arg(long, default_value_t = 10)]
    timeout: u64,

    /// Proxy URL.
    #[arg(long)]
    proxy: Option<String>,

    /// Maximum retries for transient provider/network failures.
    #[arg(long, default_value_t = 2)]
    max_retries: usize,

    /// Initial retry delay in milliseconds; later retries use linear backoff.
    #[arg(long, default_value_t = 500)]
    retry_delay_ms: u64,
}

impl ClientArgs {
    fn build_client(&self) -> websearch_rs::Result<WebSearchClient> {
        let mut builder = WebSearchClient::builder()
            .timeout(Duration::from_secs(self.timeout))
            .max_retries(self.max_retries)
            .retry_delay(Duration::from_millis(self.retry_delay_ms));

        if let Some(proxy) = &self.proxy {
            builder = builder.proxy(proxy);
        }

        builder.build()
    }
}

#[derive(Clone, Debug, Args)]
struct SearchArgs {
    /// Maximum number of results.
    #[arg(long, default_value_t = 10)]
    max_results: usize,

    /// Region code, for example wt-wt, us-en, de-de.
    #[arg(long, default_value_t = Region::default())]
    region: Region,

    /// Safe-search mode: off, moderate, on.
    #[arg(long, default_value_t = SafeSearch::default())]
    safe: SafeSearch,

    /// Backend engine selection: auto or a comma-separated list.
    #[arg(long, default_value = "auto")]
    backend: String,

    /// 1-based result page.
    #[arg(long, default_value_t = 1)]
    page: usize,

    #[command(flatten)]
    client: ClientArgs,

    /// Emit JSON instead of human-readable output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a text search.
    Text {
        /// Search query.
        query: String,

        /// Time limit: day, week, month, year.
        #[arg(long)]
        time_limit: Option<TimeLimit>,

        #[command(flatten)]
        args: SearchArgs,
    },

    /// Run an image search.
    Images {
        /// Search query.
        query: String,

        #[command(flatten)]
        args: SearchArgs,
    },

    /// Run a video search.
    Videos {
        /// Search query.
        query: String,

        /// Time limit: day, week, month, year.
        #[arg(long)]
        time_limit: Option<TimeLimit>,

        #[command(flatten)]
        args: SearchArgs,
    },

    /// Run a news search.
    News {
        /// Search query.
        query: String,

        /// Time limit: day, week, month, year.
        #[arg(long)]
        time_limit: Option<TimeLimit>,

        #[command(flatten)]
        args: SearchArgs,
    },

    /// Run a books search.
    Books {
        /// Search query.
        query: String,

        /// Maximum number of results.
        #[arg(long, default_value_t = 10)]
        max_results: usize,

        /// Backend engine selection: auto or a comma-separated list.
        #[arg(long, default_value = "auto")]
        backend: String,

        /// 1-based result page.
        #[arg(long, default_value_t = 1)]
        page: usize,

        #[command(flatten)]
        client: ClientArgs,

        /// Emit JSON instead of human-readable output.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Text {
            query,
            time_limit,
            args,
        } => {
            let client = args.client.build_client()?;
            let request = TextSearchRequest::new(query)
                .max_results(args.max_results)
                .region(args.region)
                .safe_search(args.safe)
                .time_limit(time_limit)
                .backend(args.backend)
                .page(args.page);
            let results = client.text_with(request).await?;

            if args.json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for (index, result) in results.iter().enumerate() {
                    println!("{}. {}", index + 1, result.title);
                    println!("   {}", result.href);
                    if !result.body.is_empty() {
                        println!("   {}", result.body);
                    }
                    println!();
                }
            }
        }
        Command::Images { query, args } => {
            let client = args.client.build_client()?;
            let request = ImageSearchRequest::new(query)
                .max_results(args.max_results)
                .region(args.region)
                .safe_search(args.safe)
                .backend(args.backend)
                .page(args.page);
            let results = client.images_with(request).await?;

            if args.json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for (index, result) in results.iter().enumerate() {
                    println!("{}. {}", index + 1, result.title);
                    println!("   image: {}", result.image);
                    println!("   page: {}", result.url);
                    if !result.thumbnail.is_empty() {
                        println!("   thumb: {}", result.thumbnail);
                    }
                    println!();
                }
            }
        }
        Command::Videos {
            query,
            time_limit,
            args,
        } => {
            let client = args.client.build_client()?;
            let request = VideoSearchRequest::new(query)
                .max_results(args.max_results)
                .region(args.region)
                .safe_search(args.safe)
                .time_limit(time_limit)
                .backend(args.backend)
                .page(args.page);
            let results = client.videos_with(request).await?;

            if args.json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for (index, result) in results.iter().enumerate() {
                    println!("{}. {}", index + 1, result.title);
                    println!("   {}", result.content);
                    if !result.description.is_empty() {
                        println!("   {}", result.description);
                    }
                    println!();
                }
            }
        }
        Command::News {
            query,
            time_limit,
            args,
        } => {
            let client = args.client.build_client()?;
            let request = NewsSearchRequest::new(query)
                .max_results(args.max_results)
                .region(args.region)
                .safe_search(args.safe)
                .time_limit(time_limit)
                .backend(args.backend)
                .page(args.page);
            let results = client.news_with(request).await?;

            if args.json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for (index, result) in results.iter().enumerate() {
                    println!("{}. {}", index + 1, result.title);
                    println!("   {}", result.href);
                    if !result.source.is_empty() {
                        println!("   {}", result.source);
                    }
                    if !result.body.is_empty() {
                        println!("   {}", result.body);
                    }
                    println!();
                }
            }
        }
        Command::Books {
            query,
            max_results,
            backend,
            page,
            client,
            json,
        } => {
            let client = client.build_client()?;
            let request = BooksSearchRequest::new(query)
                .max_results(max_results)
                .backend(backend)
                .page(page);
            let results = client.books_with(request).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                for (index, result) in results.iter().enumerate() {
                    println!("{}. {}", index + 1, result.title);
                    println!("   {}", result.url);
                    if !result.author.is_empty() {
                        println!("   author: {}", result.author);
                    }
                    if !result.publisher.is_empty() {
                        println!("   publisher: {}", result.publisher);
                    }
                    if !result.info.is_empty() {
                        println!("   {}", result.info);
                    }
                    println!();
                }
            }
        }
    }

    Ok(())
}
