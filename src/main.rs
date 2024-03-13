use anyhow::anyhow;
use clap::{Args, Parser};
use std::{
	collections::BTreeMap,
	io::Write,
	path::{Path, PathBuf},
	sync::{
		atomic::{AtomicUsize, Ordering},
		Arc,
	},
	time::Duration,
};
use tokio::task::JoinSet;

#[derive(Debug, clap::Parser)]
#[command(
	name = env!("CARGO_PKG_NAME"),
	version = env!("CARGO_PKG_VERSION"),
	author = env!("CARGO_PKG_AUTHORS"),
	about = env!("CARGO_PKG_DESCRIPTION"),
	rename_all_env = "lowercase",
)]
struct Cli {
	#[arg(short)]
	yes: bool,
	#[arg(short)]
	output_dir: PathBuf,
	#[arg(help = "Whitespace-separated list of tags to search for.")]
	#[arg(allow_hyphen_values = true)]
	tags: Vec<String>,
	#[arg(long)]
	#[arg(
		help = "Optional api key. Has to be specified with user_id. Can be found at https://gelbooru.com/index.php?page=account&s=options"
	)]
	api_key: Option<String>,
	#[arg(long)]
	#[arg(
		help = "Optional user id. Has to be specified with api_key. Can be found at https://gelbooru.com/index.php?page=account&s=options"
	)]
	user_id: Option<String>,
	#[arg(short = 'j')]
	#[arg(long)]
	#[arg(
		help = "Write post metadata to a JSON file. If no path is specified, writes to <OUTPUT_DIR>/posts.json.
Path is relative to <OUTPUT_DIR>.
If path is '-', writes to stderr."
	)]
	write_json: Option<Option<PathBuf>>,
	#[arg(short = 'J')]
	#[arg(long)]
	#[arg(help = "Makes the metadata JSON human-readable. Implies '--write-json'.")]
	write_pretty_json: Option<Option<PathBuf>>,

	#[clap(flatten)]
	http_args: HttpArgs,
}

#[derive(Args, Debug)]
#[group(required = false, multiple = false)]
struct HttpArgs {
	#[arg(long)]
	#[arg(short = '1')]
	#[arg(help = "Use HTTP/1.1.")]
	http1: bool,
	#[arg(short = '2')]
	#[arg(long)]
	#[arg(help = "Use HTTP/2.")]
	#[clap(action)]
	http2: bool,
	#[arg(short = '3')]
	#[arg(long)]
	#[arg(help = "Use HTTP/3. Enabled by default.")]
	#[clap(action, default_value_t = true)]
	http3: bool,
}

fn main() {
	tokio::runtime::Builder::new_multi_thread()
		.max_blocking_threads(1)
		.worker_threads(2)
		.enable_all()
		.build()
		.unwrap()
		.block_on(_main())
		.unwrap();
}

async fn _main() -> anyhow::Result<()> {
	// console_subscriber::init();

	let Cli { yes, output_dir, tags, api_key, user_id, write_json, write_pretty_json, http_args } =
		Cli::parse();

	if api_key.as_ref().xor(user_id.as_ref()).is_some() {
		return Err(anyhow!("api_key and user_id must be specified together"));
	}

	if output_dir.exists() && !output_dir.is_dir() {
		return Err(anyhow!("Not a directory: {:?}", output_dir));
	}

	println!("Searching for tags: {:?}", tags.join(" "));

	let http = match http_args {
		HttpArgs { http1: true, .. } => HttpVersion::Http1,
		HttpArgs { http2: true, .. } => HttpVersion::Http2,
		HttpArgs { http3: true, .. } => HttpVersion::Http3,
		HttpArgs { http1, http2, http3 } => {
			println!(
				"Invalid combination of arguments: http1={http1}, http2={http2}, http3={http3}"
			);
			std::process::exit(1);
		}
	};

	let client = Arc::new(GelbooruClient::new(http)?);

	let a = client.query_gelbooru(api_key.as_deref(), user_id.as_deref(), 1, 0, &tags).await;
	let GelbooruData { attributes, .. } = a?;

	if attributes.count == 0 {
		println!("No posts found.");
		return Ok(());
	}

	if !yes {
		print!("Using {http:?}. About to download {} files [Y/n]? ", attributes.count);
		std::io::stdout().flush()?;
		let mut input = String::new();
		std::io::stdin().read_line(&mut input)?;
		if input.trim().to_lowercase() == "n" {
			println!("Aborted.");
			return Ok(());
		}
	}

	if !output_dir.exists() {
		std::fs::create_dir_all(&output_dir)?;
	}

	let mut json_printer = match (write_json, write_pretty_json) {
		(Some(Some(path)), _) if path.to_string_lossy() == "-" => {
			JsonPrinter::compact(Box::new(std::io::stderr()))
		}
		(Some(path), _) => JsonPrinter::compact(Box::new(std::fs::File::create(
			output_dir.join(path.unwrap_or_else(|| "posts.json".into())),
		)?)),
		(_, Some(Some(path))) if path.to_string_lossy() == "-" => {
			JsonPrinter::pretty(Box::new(std::io::stderr()))
		}
		(_, Some(path)) => JsonPrinter::pretty(Box::new(std::fs::File::create(
			output_dir.join(path.unwrap_or_else(|| "posts.json".into())),
		)?)),
		_ => JsonPrinter::noop(),
	};

	let t1 = tokio::time::Instant::now();

	let processed = Arc::new(AtomicUsize::new(0));
	let written = Arc::new(AtomicUsize::new(0));
	let mut page = 0;
	let mut tasks = Vec::with_capacity(attributes.count);
	while let GelbooruData { posts: Some(posts), .. } =
		client.query_gelbooru(api_key.as_deref(), user_id.as_deref(), 100, page, &tags).await?
	{
		json_printer.insert_posts(&posts);

		for post in posts {
			let actual_filename = post.file_url.split('/').last().unwrap().to_owned();
			let path = output_dir.join(&actual_filename);
			if path.exists() {
				println!(
					"{}\t\talready exists {}/{}",
					actual_filename,
					processed.fetch_add(1, Ordering::Relaxed),
					attributes.count
				);
				continue;
			}
			let processed = processed.clone();
			let written = written.clone();
			let client = client.clone();
			let semaphore = client.semaphore.clone();
			let task = async move {
				let _permit = semaphore.acquire().await;

				let res = client.download_image(&post, &path).await;
				let p = processed.fetch_add(1, Ordering::Relaxed) + 1;

				match res {
					Ok(()) => {
						println!("{}\tdownloaded {}/{}", actual_filename, p, attributes.count);
						written.fetch_add(1, Ordering::Relaxed);
					}
					Err(err) => {
						println!("{}\terror {err} {}/{}", actual_filename, p, attributes.count);
					}
				}

				anyhow::Ok(())
			};
			tasks.push(task);
		}
		page += 1;
	}

	let mut joins = JoinSet::new();
	for x in tasks {
		joins.spawn(x);
	}
	while let Some(res) = joins.join_next().await {
		res??;
	}

	println!(
		"Wrote {} files. Skipped {}. Time taken: {:.2?}",
		written.load(Ordering::Relaxed),
		processed.load(Ordering::Relaxed) - written.load(Ordering::Relaxed),
		t1.elapsed()
	);

	json_printer.write()?;

	Ok(())
}

struct GelbooruClient {
	image_client: reqwest::Client,
	video_client: reqwest::Client,
	semaphore: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone, Copy, Debug)]
enum HttpVersion {
	Http1,
	Http2,
	Http3,
}

impl GelbooruClient {
	fn new(http: HttpVersion) -> anyhow::Result<Self> {
		let image_client = reqwest::Client::builder()
			.user_agent(concat!(std::env!("CARGO_PKG_NAME"), "/", std::env!("CARGO_PKG_VERSION")))
			.tcp_keepalive(Some(Duration::from_secs(60)))
			.danger_accept_invalid_certs(true);
		let image_client = match http {
			HttpVersion::Http1 => image_client,
			HttpVersion::Http2 => image_client.http2_prior_knowledge(),
			HttpVersion::Http3 => image_client.http3_prior_knowledge(),
		};

		let video_client = reqwest::Client::builder()
			.user_agent(concat!(std::env!("CARGO_PKG_NAME"), "/", std::env!("CARGO_PKG_VERSION")))
			.tcp_keepalive(Some(Duration::from_secs(60)))
			.danger_accept_invalid_certs(true);
		Ok(GelbooruClient {
			image_client: image_client.build()?,
			video_client: video_client.build()?,
			semaphore: Arc::new(tokio::sync::Semaphore::new(24)),
		})
	}

	async fn query_gelbooru(
		&self,
		api_key: Option<&str>,
		user_id: Option<&str>,
		limit: usize,
		page: usize,
		tags: &[String],
	) -> anyhow::Result<GelbooruData> {
		let tags = tags.join(" ");
		Ok(self
			.image_client
			.get("https://gelbooru.com/index.php?page=dapi&s=post&q=index&json=1")
			.query(&[("limit", limit.to_string()), ("pid", page.to_string()), ("tags", tags)])
			.query(&[
				("api_key", api_key.unwrap_or_default()),
				("user_id", user_id.unwrap_or_default()),
			])
			.send()
			.await
			.map_err(|x| anyhow!("{x} at page {}", page))?
			.json::<GelbooruData>()
			.await?)
	}

	async fn download_image(&self, post: &GelbooruPost, path: &Path) -> anyhow::Result<()> {
		let client = if post.image.ends_with(".webm") || post.image.ends_with(".mp4") {
			&self.video_client
		} else {
			&self.image_client
		};
		let bytes = client
			.get(&post.file_url)
			.send()
			.await?
			.bytes()
			.await
			.map_err(|x| anyhow!("{x} at {}", post.file_url))?;
		tokio::fs::write(path, bytes).await?;
		Ok(())
	}
}

#[derive(serde::Deserialize)]
struct GelbooruData {
	#[serde(rename = "@attributes")]
	attributes: GelbooruAttributes,
	#[serde(rename = "post")]
	posts: Option<Vec<GelbooruPost>>,
}

#[derive(serde::Deserialize)]
struct GelbooruAttributes {
	// limit: usize,
	// offset: usize,
	count: usize,
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct GelbooruPost {
	pub id: i64,
	pub created_at: String,
	pub score: i64,
	pub width: i64,
	pub height: i64,
	pub md5: String,
	pub directory: String,
	pub image: String,
	pub rating: String,
	pub source: String,
	pub change: i64,
	pub owner: String,
	pub creator_id: i64,
	pub parent_id: i64,
	pub sample: i64,
	pub preview_height: i64,
	pub preview_width: i64,
	pub tags: String,
	pub title: String,
	pub has_notes: String,
	pub has_comments: String,
	pub file_url: String,
	pub preview_url: String,
	pub sample_url: String,
	pub sample_height: i64,
	pub sample_width: i64,
	pub status: String,
	pub post_locked: i64,
	pub has_children: String,
}

enum JsonPrinter {
	Compact(Box<dyn Write>, BTreeMap<String, GelbooruPost>),
	Pretty(Box<dyn Write>, BTreeMap<String, GelbooruPost>),
	NoOp,
}

impl JsonPrinter {
	fn compact(file: Box<dyn Write>) -> Self {
		Self::Compact(file, BTreeMap::default())
	}

	fn pretty(file: Box<dyn Write>) -> Self {
		Self::Pretty(file, BTreeMap::default())
	}

	fn noop() -> Self {
		Self::NoOp
	}

	fn insert_posts(&mut self, posts: &[GelbooruPost]) {
		match self {
			Self::Pretty(_, map) | Self::Compact(_, map) => {
				map.extend(posts.iter().map(|post| (post.md5.clone(), post.clone())));
			}
			Self::NoOp => {}
		}
	}

	fn write(self) -> anyhow::Result<()> {
		match self {
			Self::Compact(file, posts) => {
				serde_json::to_writer(file, &posts)?;
			}
			Self::Pretty(file, posts) => {
				serde_json::to_writer_pretty(file, &posts)?;
			}
			Self::NoOp => {}
		}
		Ok(())
	}
}
