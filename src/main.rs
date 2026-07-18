#![allow(unstable_name_collisions)]
pub mod api;
pub mod models;
pub mod rss;
pub mod sitemap;
pub mod web;

use axum::{Router, http::HeaderMap, routing::*};
use meilisearch_sdk::client::*;
use models::*;
use sqlx::postgres::PgPoolOptions;

#[derive(Clone)]
pub struct Config {
	pub decoding_key: jsonwebtoken::DecodingKey,
	pub encoding_key: jsonwebtoken::EncodingKey,
	pub discord_id: String,
	pub discord_secret: String,
	pub discord_bot_token: String,
	pub cloudflare_image_token: String,
	pub cloudflare_account_id: String,
	pub admins: Vec<i64>,
	pub storage_path: String,
	pub rclone_path: String,
}

#[derive(Clone)]
pub struct AppState {
	pub config: Config,
	pub db: sqlx::Pool<sqlx::Postgres>,
	pub meilisearch: Client,
}

#[tokio::main]
async fn main() {
	env_logger::init();
	dotenvy::dotenv().expect(".env must exist");

	let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must exist");
	let db = PgPoolOptions::new()
		.min_connections(4)
		.max_connections(32)
		.connect(&database_url)
		.await
		.expect("Could not connect to database");
	sqlx::migrate!()
		.run(&db)
		.await
		.expect("Unable to run migrations");

	let secret_key = std::env::var("SECRET_KEY").expect("SECRET_KEY must exist");
	let decoding_key = jsonwebtoken::DecodingKey::from_secret(secret_key.as_bytes());
	let encoding_key = jsonwebtoken::EncodingKey::from_secret(secret_key.as_bytes());

	let discord_id = std::env::var("DISCORD_ID").expect("DISCORD_ID must exist");
	let discord_secret = std::env::var("DISCORD_SECRET").expect("DISCORD_SECRET must exist");
	let discord_bot_token =
		std::env::var("DISCORD_BOT_TOKEN").expect("DISCORD_BOT_TOKEN must exist");

	let cloudflare_image_token =
		std::env::var("CLOUDFLARE_IMAGE_TOKEN").expect("CLOUDFLARE_IMAGE_TOKEN must exist");
	let cloudflare_account_id =
		std::env::var("CLOUDFLARE_ACCOUNT_ID").expect("CLOUDFLARE_ACCOUNT_ID must exist");

	let admins = std::env::var("ADMIN_IDS")
		.expect("ADMIN_IDS must exist")
		.split(',')
		.map(|x| x.parse::<i64>().expect("Admin IDs must be i64"))
		.collect();

	let meilisearch_url = std::env::var("MEILISEARCH_URL").expect("MEILISEARCH_URL must exist");
	let storage_path = std::env::var("STORAGE_PATH").expect("STORAGE_PATH must exist");
	let rclone_path = std::env::var("RCLONE_PATH").expect("RCLONE_PATH must exist");

	let port = std::env::var("PORT")
		.unwrap_or(String::from("7001"))
		.parse::<i64>()
		.unwrap_or(7001);

	let config = Config {
		decoding_key,
		encoding_key,
		discord_id,
		discord_secret,
		discord_bot_token,
		cloudflare_image_token,
		cloudflare_account_id,
		admins,
		storage_path,
		rclone_path,
	};

	let client = meilisearch_sdk::client::Client::new(meilisearch_url, None::<&str>).unwrap();

	let meilisearch_posts = client.index("posts");
	meilisearch_posts
		.set_searchable_attributes(&["authors.name", "name", "text"])
		.await
		.unwrap();
	meilisearch_posts
		.set_filterable_attributes(&["post_type", "post_game", "id", "private", "time"])
		.await
		.unwrap();
	meilisearch_posts
		.set_sortable_attributes(&["download_count", "like_count", "time"])
		.await
		.unwrap();

	let posts = sqlx::query!("SELECT id FROM posts ORDER BY time DESC")
		.fetch_all(&db)
		.await;

	if let Ok(posts) = posts {
		let mut vec = Vec::with_capacity(posts.len());
		for post in &posts {
			let Some(post) = Post::get_short(post.id, &db).await else {
				continue;
			};
			vec.push(post);
		}
		meilisearch_posts.add_or_update(&vec, None).await.unwrap();
	}

	let state = AppState {
		config,
		db,
		meilisearch: client,
	};

	let cloned_state = state.clone();
	std::thread::spawn(|| routine_tasks(cloned_state));

	let router = Router::new()
		.route("/robots.txt", get(robots))
		.route("/favicon.ico", get(favicon))
		.route("/dma_black.png", get(dma_black))
		.route("/sitemap.xml", get(sitemap::sitemap))
		.route("/rss.xml", get(rss::rss))
		.route("/login", get(login))
		.fallback(not_found)
		.layer(axum::extract::DefaultBodyLimit::disable())
		.layer(
			tower_http::compression::CompressionLayer::new()
				.gzip(true)
				.deflate(true)
				.br(true)
				.zstd(true),
		)
		.with_state(state.clone())
		.merge(web::route(state.clone()))
		.merge(api::route(state.clone()));

	let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
		.await
		.expect(&format!("Unable to bind on 0.0.0.0:{port}"));
	axum::serve(listener, router).await.unwrap();
}

pub async fn robots() -> &'static str {
	"User-agent: *\nDisallow: /api/\nSitemap: https://taikomodarchive.com/sitemap.xml"
}

pub async fn favicon() -> (HeaderMap, &'static [u8]) {
	let mut headers = HeaderMap::new();
	headers.insert("content-type", "image/vnd.microsoft.icon".parse().unwrap());
	(headers, include_bytes!("../static/DMA_BLACK_STARLESS.ico"))
}

pub async fn dma_black() -> (HeaderMap, &'static [u8]) {
	let mut headers = HeaderMap::new();
	headers.insert("content-type", "image/png".parse().unwrap());
	(headers, include_bytes!("../static/DMA_BLACK.png"))
}

pub async fn not_found(base: web::BaseTemplate) -> ErrorTemplate {
	ErrorTemplate {
		base,
		status: reqwest::StatusCode::NOT_FOUND,
	}
}
