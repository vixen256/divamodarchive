use crate::AppState;
use axum::{extract::*, http::HeaderMap, response::*};
use reqwest::{StatusCode, header};
use serde::Serialize;

#[derive(Serialize)]
struct Enclosure {
	#[serde(rename = "@url")]
	url: String,
	#[serde(rename = "@length")]
	length: u64,
	#[serde(rename = "@type")]
	media_type: String,
}

#[derive(Serialize)]
struct Item {
	title: String,
	description: String,
	link: String,
	#[serde(rename = "pubDate")]
	pub_date: String,
	enclosure: Option<Enclosure>,
}

#[derive(Serialize)]
struct Atom {
	#[serde(rename = "@href")]
	href: String,
	#[serde(rename = "@rel")]
	rel: String,
	#[serde(rename = "@type")]
	atom_type: String,
}

#[derive(Serialize)]
struct Channel {
	title: String,
	description: String,
	link: String,
	#[serde(rename = "lastBuildDate")]
	last_build_date: String,
	ttl: i32,
	#[serde(rename = "atom:link")]
	atom: Atom,
	item: Vec<Item>,
}

#[derive(Serialize)]
#[serde(rename = "rss")]
struct Rss {
	#[serde(rename = "@version")]
	version: String,
	#[serde(rename = "@xmlns:atom")]
	xmlns: String,
	channel: Vec<Channel>,
}

pub async fn rss(State(state): State<AppState>) -> Result<(HeaderMap, String), StatusCode> {
	let mut last_build_date = time::PrimitiveDateTime::MIN;

	let posts = sqlx::query!(
		"SELECT id, time, name, text, images FROM posts WHERE private = false ORDER BY time DESC"
	)
	.fetch_all(&state.db)
	.await
	.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

	let mut set = tokio::task::JoinSet::new();
	for post in posts {
		if post.time > last_build_date {
			last_build_date = post.time;
		}

		set.spawn(async move {
			let enclosure = if let Some(image) = post.images.first() {
				let image = image.replace("/public", "/thumbnail");
				let client = reqwest::Client::new();
				if let Ok(res) = client.head(&image).send().await {
					let length = res
						.headers()
						.get(reqwest::header::CONTENT_LENGTH)
						.map(|v| v.to_str().unwrap_or("0").parse().unwrap_or(0u64))
						.unwrap_or(0);
					let media_type = res
						.headers()
						.get(reqwest::header::CONTENT_TYPE)
						.map(|v| v.to_str().unwrap_or("image/avif"))
						.unwrap_or("image/avif");

					Some(Enclosure {
						url: image,
						length,
						media_type: String::from(media_type),
					})
				} else {
					None
				}
			} else {
				None
			};

			(
				post.time,
				Item {
					title: post.name,
					description: String::from(
						post.text
							.lines()
							.next()
							.unwrap_or_default()
							.replace('<', "&lt;")
							.replace('>', "&rt;"),
					),
					link: format!("https://divamodarchive.com/posts/{}", post.id),
					pub_date: post
						.time
						.assume_offset(time::UtcOffset::UTC)
						.format(&time::format_description::well_known::Rfc2822)
						.unwrap(),
					enclosure,
				},
			)
		});
	}

	let mut items = set.join_all().await;
	items.sort_by(|(a, _), (b, _)| b.cmp(a));

	let xml = Rss {
		channel: vec![Channel {
			title: String::from("DMA mods"),
			description: String::from("Mods"),
			link: String::from("https://divamodarchive.com"),
			last_build_date: last_build_date
				.assume_offset(time::UtcOffset::UTC)
				.format(&time::format_description::well_known::Rfc2822)
				.unwrap(),
			ttl: 60 * 60 * 24,
			atom: Atom {
				href: String::from("https://divamodarchive.com/rss.xml"),
				rel: String::from("self"),
				atom_type: String::from("application/rss+xml"),
			},
			item: items.into_iter().map(|(_, item)| item).collect(),
		}],
		xmlns: String::from("http://www.w3.org/2005/Atom"),
		version: String::from("2.0"),
	};
	let xml = quick_xml::se::to_string(&xml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

	let mut headers = HeaderMap::new();
	headers.insert(header::CONTENT_TYPE, "application/xml".parse().unwrap());

	Ok((
		headers,
		format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{xml}"),
	))
}
