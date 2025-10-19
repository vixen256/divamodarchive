use crate::AppState;
use crate::api::ids::*;
use crate::models::*;
use axum::{
	extract::*,
	http::{StatusCode, header},
	response::*,
};
use base64::prelude::*;
use itertools::*;
use serde::{Deserialize, Serialize};
use std::collections::*;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use utoipa::{IntoParams, ToSchema};

#[derive(Serialize, Deserialize)]
struct CloudflareDirectUploadResult {
	id: String,
	#[serde(rename = "uploadURL")]
	upload_url: String,
}

#[derive(Serialize, Deserialize)]
pub struct CloudflareMessage {
	code: usize,
	message: String,
	documentation_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CloudflareEmpty {}

#[derive(Serialize, Deserialize)]
struct CloudflareApiResponse<T> {
	errors: Vec<CloudflareMessage>,
	messages: Vec<CloudflareMessage>,
	success: bool,
	result: T,
}

pub async fn upload_image(_: User, State(state): State<AppState>) -> Result<String, StatusCode> {
	let cloudflare_url = format!(
		"https://api.cloudflare.com/client/v4/accounts/{}/images/v2/direct_upload",
		state.config.cloudflare_account_id
	);

	let response = reqwest::Client::new()
		.post(&cloudflare_url)
		.header(
			header::AUTHORIZATION.to_string(),
			format!("Bearer {}", state.config.cloudflare_image_token),
		)
		.send()
		.await;

	let response = match response {
		Ok(response) => response,
		Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
	};
	if !response.status().is_success() {
		return Err(StatusCode::INTERNAL_SERVER_ERROR);
	}
	let response = response
		.json::<CloudflareApiResponse<CloudflareDirectUploadResult>>()
		.await;
	let response = match response {
		Ok(response) => response,
		Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
	};
	if response.success {
		Ok(response.result.upload_url)
	} else {
		Err(StatusCode::INTERNAL_SERVER_ERROR)
	}
}

pub async fn remove_image(
	Path((post_id, index)): Path<(i32, usize)>,
	user: User,
	State(state): State<AppState>,
) -> Result<(), StatusCode> {
	let Some(post) = Post::get_short(post_id, &state.db).await else {
		return Err(StatusCode::BAD_REQUEST);
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}

	if index >= post.images.len() {
		return Err(StatusCode::BAD_REQUEST);
	}

	let mut images = post.images;
	let old_image = images.remove(index);
	if reqwest::Client::new()
		.head(&old_image)
		.send()
		.await
		.map_or(false, |res| res.status().is_success())
	{
		let Some(old_image) = old_image.trim_end_matches("/public").split("/").last() else {
			return Err(StatusCode::INTERNAL_SERVER_ERROR);
		};

		let cloudflare_url = format!(
			"https://api.cloudflare.com/client/v4/accounts/{}/images/v1/{}",
			state.config.cloudflare_account_id, old_image
		);

		let Ok(response) = reqwest::Client::new()
			.delete(&cloudflare_url)
			.header(
				header::AUTHORIZATION.to_string(),
				format!("Bearer {}", state.config.cloudflare_image_token),
			)
			.send()
			.await
		else {
			return Err(StatusCode::INTERNAL_SERVER_ERROR);
		};

		if !response.status().is_success() {
			return Err(StatusCode::INTERNAL_SERVER_ERROR);
		}

		let Ok(response) = response
			.json::<CloudflareApiResponse<CloudflareEmpty>>()
			.await
		else {
			return Err(StatusCode::INTERNAL_SERVER_ERROR);
		};
		if !response.success {
			return Err(StatusCode::INTERNAL_SERVER_ERROR);
		}
	}

	_ = sqlx::query!("UPDATE posts SET images=$1 WHERE id=$2", &images, post_id)
		.execute(&state.db)
		.await;

	Ok(())
}

pub async fn append_image(
	Path(post_id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(image): Json<String>,
) -> StatusCode {
	let Some(post) = Post::get_short(post_id, &state.db).await else {
		return StatusCode::BAD_REQUEST;
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return StatusCode::UNAUTHORIZED;
	}

	if !image.starts_with("https://divamodarchive.com/cdn-cgi/imagedelivery")
		|| !image.ends_with("/public")
		|| reqwest::get(&image).await.is_err()
	{
		return StatusCode::BAD_REQUEST;
	}

	let mut images = post.images;
	images.push(image);

	_ = sqlx::query!("UPDATE posts SET images=$1 WHERE id=$2", &images, post_id)
		.execute(&state.db)
		.await;

	StatusCode::OK
}

#[derive(Serialize, Deserialize)]
pub struct SwapImages {
	pub from: usize,
	pub to: usize,
}

pub async fn swap_images(
	Path(post_id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(data): Json<SwapImages>,
) -> Result<(), StatusCode> {
	let Some(post) = Post::get_short(post_id, &state.db).await else {
		return Err(StatusCode::BAD_REQUEST);
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}

	if data.from >= post.images.len() || data.to >= post.images.len() {
		return Err(StatusCode::BAD_REQUEST);
	}

	let mut images = post.images;
	images.swap(data.from, data.to);

	_ = sqlx::query!("UPDATE posts SET images=$1 WHERE id=$2", &images, post_id)
		.execute(&state.db)
		.await;

	Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct PostEditData {
	pub name: String,
	pub text: String,
	pub post_type: i32,
	pub private: bool,
	pub explicit: bool,
	pub explicit_reason: String,
}

pub async fn edit_post(
	user: User,
	Path(id): Path<i32>,
	State(state): State<AppState>,
	Json(data): Json<PostEditData>,
) -> Result<(), StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::BAD_REQUEST);
	};
	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(StatusCode::BAD_REQUEST);
	}
	if !data.private && post.files.is_empty() {
		return Err(StatusCode::BAD_REQUEST);
	}

	let explicit_reason = if !data.explicit || data.explicit_reason.is_empty() {
		None
	} else {
		Some(data.explicit_reason)
	};

	sqlx::query!(
		"UPDATE posts SET name = $2, text = $3, type = $4, private = $5, explicit = $6, explicit_reason = $7 WHERE id = $1",
		id,
		data.name,
		data.text,
		data.post_type,
		data.private,
		data.explicit,
		explicit_reason
	)
	.execute(&state.db)
	.await
	.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

	if let Some(post) = Post::get_short(id, &state.db).await {
		_ = state
			.meilisearch
			.index("posts")
			.add_or_update(&[post], None)
			.await;
	};

	Ok(())
}

pub async fn get_download_link(filepath: &str) -> Option<String> {
	let command = tokio::process::Command::new("rclone")
		.arg("link")
		.arg(format!("pixeldrainfs:/divamodarchive/{}", filepath))
		.arg("--config=/etc/rclone-mnt.conf")
		.output()
		.await;
	let Ok(command) = command else {
		return None;
	};
	if !command.status.success() {
		return None;
	}
	let Ok(path) = String::from_utf8(command.stdout) else {
		return None;
	};

	if !path.starts_with("https://pixeldrain.com/d/") {
		return None;
	}

	let download = path.trim().replace(
		"https://pixeldrain.com/d/",
		"https://pixeldrain.com/api/filesystem/",
	);
	Some(format!("{download}?download"))
}

#[derive(Serialize, Deserialize)]
pub struct CreatePendingUpload {
	pub post: i32,
	pub files: Vec<String>,
	pub lengths: Vec<i64>,
}

pub async fn create_pending_upload(
	user: User,
	State(state): State<AppState>,
	Json(upload_data): Json<CreatePendingUpload>,
) -> StatusCode {
	if upload_data.lengths.len() != upload_data.files.len()
		|| upload_data.lengths.is_empty()
		|| upload_data.lengths.iter().any(|length| *length <= 0)
	{
		return StatusCode::BAD_REQUEST;
	}

	let Some(post) = Post::get_short(upload_data.post, &state.db).await else {
		return StatusCode::BAD_REQUEST;
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return StatusCode::UNAUTHORIZED;
	}

	if sqlx::query!("SELECT * FROM pending_uploads WHERE user_id = $1", user.id)
		.fetch_optional(&state.db)
		.await
		.unwrap_or_default()
		.is_some()
	{
		return StatusCode::CONFLICT;
	}

	_ = tokio::fs::create_dir_all(format!("/pixeldrain/{}/pending", user.id)).await;
	for file in &upload_data.files {
		if file.contains('/') {
			return StatusCode::BAD_REQUEST;
		}
		let path = format!("{}/{file}", user.id);
		if !post.local_files.contains(&path)
			&& std::path::Path::new(&format!("/pixeldrain/{path}")).exists()
		{
			return StatusCode::CONFLICT;
		}

		if tokio::fs::File::create(&format!("/pixeldrain/{}/pending/{file}", user.id))
			.await
			.is_err()
		{
			return StatusCode::INTERNAL_SERVER_ERROR;
		};
	}

	if sqlx::query!(
		"INSERT INTO pending_uploads (files, completed, length, post_id, user_id) VALUES ($1, $2, $3, $4, $5)",
		&upload_data.files,
		&upload_data.files.iter().map(|_| 0).collect::<Vec<_>>(),
		&upload_data.lengths,
		post.id,
		user.id
	)
	.execute(&state.db)
	.await
	.is_err()
	{
		return StatusCode::INTERNAL_SERVER_ERROR;
	};

	StatusCode::OK
}

pub async fn continue_pending_upload(
	ws: ws::WebSocketUpgrade,
	State(state): State<AppState>,
) -> Response {
	ws.on_upgrade(move |socket| continue_pending_upload_ws(socket, state))
}

pub async fn continue_pending_upload_ws(mut socket: ws::WebSocket, state: AppState) {
	let Some(Ok(ws::Message::Text(message))) = socket.recv().await else {
		_ = socket
			.send(ws::Message::Text(ws::Utf8Bytes::from_static(
				"{\"error\": \"Failed to recv message\"}",
			)))
			.await;
		return;
	};

	let Ok(user) = User::parse(&message.trim(), &state).await else {
		_ = socket
			.send(ws::Message::Text(ws::Utf8Bytes::from_static(
				"{\"error\": \"Failed to authenticate\"}",
			)))
			.await;
		return;
	};

	let Ok(mut pending_upload) = sqlx::query!(
		"SELECT files, completed, length, post_id FROM pending_uploads WHERE user_id = $1",
		user.id
	)
	.fetch_one(&state.db)
	.await
	else {
		_ = socket
			.send(ws::Message::Text(ws::Utf8Bytes::from_static(
				"{\"error\": \"Failed to get pending upload\"}",
			)))
			.await;
		return;
	};

	let Some(post) = Post::get_short(pending_upload.post_id, &state.db).await else {
		_ = socket
			.send(ws::Message::Text(ws::Utf8Bytes::from_static(
				"{\"error\": \"Failed to get post\"}",
			)))
			.await;
		return;
	};

	let local_files = pending_upload
		.files
		.iter()
		.map(|file| format!("/pixeldrain/{}/pending/{file}", user.id))
		.collect::<Vec<_>>();
	for (i, file_name) in pending_upload.files.iter().enumerate() {
		let Ok(mut file) = tokio::fs::OpenOptions::new()
			.write(true)
			.open(&local_files[i])
			.await
		else {
			if tokio::fs::try_exists(format!("/pixeldrain/{}/{file_name}", user.id))
				.await
				.map_or(false, |exists| exists)
			{
				continue;
			}
			_ = socket
				.send(ws::Message::Text(ws::Utf8Bytes::from_static(
					"{\"error\": \"Failed to open file\"}",
				)))
				.await;
			return;
		};
		_ = file
			.seek(std::io::SeekFrom::Start(pending_upload.completed[i] as u64))
			.await;

		while pending_upload.completed[i] < pending_upload.length[i] {
			if socket
				.send(ws::Message::Text(
					format!(
						"{{ \"file\": \"{file_name}\", \"offset\": {} }}",
						pending_upload.completed[i]
					)
					.into(),
				))
				.await
				.is_err()
			{
				// Likely wont ever work since the socket is likely closed here
				_ = socket
					.send(ws::Message::Text(ws::Utf8Bytes::from_static(
						"{\"error\": \"Failed to send message\"}",
					)))
					.await;
				return;
			}

			let Some(Ok(ws::Message::Binary(data))) = socket.recv().await else {
				_ = socket
					.send(ws::Message::Text(ws::Utf8Bytes::from_static(
						"{\"error\": \"Failed to recv message\"}",
					)))
					.await;
				return;
			};

			if file.write_all(&data).await.is_err() {
				_ = socket
					.send(ws::Message::Text(ws::Utf8Bytes::from_static(
						"{\"error\": \"Failed to write data\"}",
					)))
					.await;
				return;
			};

			if file.sync_data().await.is_err() {
				_ = socket
					.send(ws::Message::Text(ws::Utf8Bytes::from_static(
						"{\"error\": \"Failed to write data\"}",
					)))
					.await;
				return;
			}

			pending_upload.completed[i] += data.len() as i64;

			if sqlx::query!(
				"UPDATE pending_uploads SET completed = $1 WHERE user_id = $2",
				&pending_upload.completed,
				user.id
			)
			.execute(&state.db)
			.await
			.is_err()
			{
				_ = socket
					.send(ws::Message::Text(ws::Utf8Bytes::from_static(
						"{\"error\": \"Failed to update database\"}",
					)))
					.await;
				return;
			}
		}
	}

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("pvs"))
		.with_filter(&format!("post={}", post.id))
		.execute::<MeilisearchPv>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("modules"))
		.with_filter(&format!("post_id={}", post.id))
		.execute::<MeilisearchModule>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("cstm_items"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchCstmItem>()
	.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("nc_songs"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchNcSong>()
	.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("sprites"))
		.with_filter(&format!("post_id={}", post.id))
		.execute::<MeilisearchDbEntry>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("aets"))
		.with_filter(&format!("post_id={}", post.id))
		.execute::<MeilisearchDbEntry>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("objsets"))
		.with_filter(&format!("post_id={}", post.id))
		.execute::<MeilisearchDbEntry>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("textures"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchDbEntry>()
	.await;

	let mut pending_exists = false;
	for file in &pending_upload.files {
		if !tokio::fs::try_exists(format!("/pixeldrain/{}/pending/{file}", user.id))
			.await
			.map_or(false, |exists| exists)
		{
			continue;
		}

		pending_exists = true;
	}

	if pending_exists {
		for file in post.local_files.iter() {
			_ = tokio::process::Command::new("rclone")
				.arg("delete")
				.arg(format!("pixeldrainfs:/divamodarchive/{}", file))
				.arg("--config=/etc/rclone-mnt.conf")
				.output()
				.await;
		}

		for file in &pending_upload.files {
			_ = tokio::process::Command::new("rclone")
				.arg("moveto")
				.arg(format!(
					"pixeldrainfs:/divamodarchive/{}/pending/{}",
					user.id, file
				))
				.arg(format!("pixeldrainfs:/divamodarchive/{}/{}", user.id, file))
				.arg("--config=/etc/rclone-mnt.conf")
				.output()
				.await;
		}
	}

	let files = pending_upload
		.files
		.iter()
		.map(|file| format!("{}/{file}", user.id))
		.collect::<Vec<_>>();

	let mut downloads = Vec::new();
	for file in &files {
		loop {
			let Some(download) = get_download_link(file).await else {
				tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
				continue;
			};
			if download.is_empty() {
				tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
				continue;
			}
			downloads.push(download);
			break;
		}
	}

	_ = socket
		.send(ws::Message::Text(ws::Utf8Bytes::from_static(
			"{\"success\": \"\"}",
		)))
		.await;

	let Some(Ok(ws::Message::Text(data))) = socket.recv().await else {
		_ = socket
			.send(ws::Message::Text(ws::Utf8Bytes::from_static(
				"{\"error\": \"Failed to recv message\"}",
			)))
			.await;
		return;
	};

	let Ok(data) = serde_json::from_str::<PostEditData>(data.as_str()) else {
		_ = socket
			.send(ws::Message::Text(ws::Utf8Bytes::from_static(
				"{\"error\": \"Failed to parse message\"}",
			)))
			.await;
		return;
	};

	let explicit_reason = if !data.explicit || data.explicit_reason.is_empty() {
		None
	} else {
		Some(data.explicit_reason)
	};

	let now = time::OffsetDateTime::now_utc();
	let time = time::PrimitiveDateTime::new(now.date(), now.time());

	_ = sqlx::query!(
		"UPDATE posts SET files = $2, local_files = $3, time = $4, name = $5, text = $6, type = $7, private = $8, explicit = $9, explicit_reason = $10 WHERE id = $1",
		post.id,
		&downloads,
		&files,
		time,
		data.name,
		data.text,
		data.post_type,
		data.private,
		data.explicit,
		explicit_reason
	)
	.execute(&state.db)
	.await;

	_ = sqlx::query!("DELETE FROM pending_uploads WHERE user_id = $1", user.id)
		.execute(&state.db)
		.await;

	if let Some(post) = Post::get_short(post.id, &state.db).await {
		_ = state
			.meilisearch
			.index("posts")
			.add_or_update(&[post], None)
			.await;
	};

	_ = tokio::spawn(extract_post_data(post.id, state.clone())).await;
}

#[derive(Serialize, Deserialize)]
pub struct PostCreationData {
	pub name: String,
	pub text: String,
	pub post_type: i32,
	pub images: Vec<String>,
}

pub async fn create_post(
	user: User,
	State(state): State<AppState>,
	Json(data): Json<PostCreationData>,
) -> Result<Json<Post>, StatusCode> {
	for image in &data.images {
		if !image.starts_with("https://divamodarchive.com/cdn-cgi/imagedelivery")
			|| reqwest::get(image).await.is_err()
		{
			return Err(StatusCode::BAD_REQUEST);
		}
	}

	if sqlx::query!(
		"SELECT p.id
		FROM post_authors pa
		LEFT JOIN posts p ON pa.post_id = p.id
		WHERE pa.user_id = $1
		AND p.files = '{}'
		LIMIT 1",
		user.id
	)
	.fetch_optional(&state.db)
	.await
	.unwrap_or_default()
	.is_some()
	{
		return Err(StatusCode::CONFLICT);
	}

	let Ok(id) = sqlx::query!(
		"
		INSERT INTO posts (name, text, images, type, time, files, local_files, private)
		VALUES ($1, $2, $3, $4, '1970-01-01', '{}', '{}', true)
		RETURNING ID
		",
		data.name,
		data.text,
		&data.images,
		data.post_type
	)
	.fetch_one(&state.db)
	.await
	else {
		return Err(StatusCode::INTERNAL_SERVER_ERROR);
	};

	_ = sqlx::query!(
		"INSERT INTO post_authors (post_id, user_id) VALUES ($1, $2)",
		id.id,
		user.id,
	)
	.execute(&state.db)
	.await;

	let Some(post) = Post::get_short(id.id, &state.db).await else {
		return Err(StatusCode::INTERNAL_SERVER_ERROR);
	};

	Ok(Json(post))
}

pub async fn extract_post(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
) -> StatusCode {
	if !user.is_admin(&state.config) {
		return StatusCode::UNAUTHORIZED;
	}

	tokio::spawn(extract_post_data(id, state.clone()));

	StatusCode::OK
}

pub async fn download(
	Path((id, variant)): Path<(i32, i32)>,
	State(state): State<AppState>,
) -> Result<Redirect, StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	_ = sqlx::query!(
		"UPDATE posts SET download_count = download_count +1 WHERE id = $1",
		id
	)
	.execute(&state.db)
	.await;

	if let Some(post) = Post::get_short(id, &state.db).await {
		_ = state
			.meilisearch
			.index("posts")
			.add_or_update(&[post], None)
			.await;
	};

	let Some(file) = post.files.get(variant as usize) else {
		return Err(StatusCode::BAD_REQUEST);
	};

	Ok(Redirect::to(file))
}

pub async fn download_head(
	Path((id, variant)): Path<(i32, i32)>,
	State(state): State<AppState>,
) -> Result<Redirect, StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	let Some(file) = post.files.get(variant as usize) else {
		return Err(StatusCode::BAD_REQUEST);
	};

	Ok(Redirect::to(file))
}

pub async fn like(Path(id): Path<i32>, user: User, State(state): State<AppState>) -> StatusCode {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return StatusCode::NOT_FOUND;
	};

	let Ok(has_liked) = sqlx::query!(
		"SELECT COUNT(*) FROM liked_posts WHERE post_id = $1 AND user_id = $2",
		post.id,
		user.id
	)
	.fetch_one(&state.db)
	.await
	else {
		return StatusCode::INTERNAL_SERVER_ERROR;
	};

	if has_liked.count.unwrap_or(0) > 0 {
		_ = sqlx::query!(
			"DELETE FROM liked_posts WHERE post_id = $1 AND user_id = $2",
			post.id,
			user.id
		)
		.execute(&state.db)
		.await;
	} else {
		_ = sqlx::query!(
			"INSERT INTO liked_posts (post_id, user_id) VALUES ($1, $2)",
			post.id,
			user.id
		)
		.execute(&state.db)
		.await;
	}

	if let Some(post) = Post::get_short(id, &state.db).await {
		_ = state
			.meilisearch
			.index("posts")
			.add_or_update(&[post], None)
			.await;
	};

	StatusCode::OK
}

#[utoipa::path(
	get,
	path = "/api/v1/posts/{id}",
	params(
		("id" = i32, Path)
	),
	responses(
		(status = 200, body = Post, content_type = "application/json"),
		(status = 401),
		(status = 404)
	)
)]
pub async fn get_post(
	Path(id): Path<i32>,
	State(state): State<AppState>,
) -> Result<Json<Post>, StatusCode> {
	let Some(mut post) = Post::get_full(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	if post.private {
		return Err(StatusCode::UNAUTHORIZED);
	}

	for i in 0..post.files.len() {
		post.files[i] = format!(
			"https://divamodarchive.com/api/v1/posts/{}/download/{i}",
			post.id
		);
		post.local_files[i] = post.local_files[i]
			.split("/")
			.last()
			.map(|s| String::from(s))
			.unwrap_or(String::new());
	}
	Ok(Json(post))
}

#[derive(Serialize, Deserialize)]
pub struct MultiplePostsParams {
	pub post_id: Vec<i32>,
}

pub async fn get_multiple_posts(
	Query(posts): Query<MultiplePostsParams>,
	State(state): State<AppState>,
) -> Result<Json<Vec<Post>>, (StatusCode, String)> {
	let filter = posts
		.post_id
		.iter()
		.map(|id| format!("id={id}"))
		.collect::<Vec<_>>()
		.join(" OR ");
	let params = SearchParams {
		query: None,
		sort: None,
		filter: Some(filter),
		limit: Some(posts.post_id.len()),
		offset: None,
	};
	search_posts(Query(params), State(state)).await
}

#[derive(Serialize, Deserialize, Clone, IntoParams)]
pub struct SearchParams {
	pub query: Option<String>,
	/// One of time:desc, time:asc, like_count:desc, download_count:desc
	pub sort: Option<String>,
	/**
	A meilisearch filter, such as `post_type = Plugin AND id != 100`
	Attributes: post_type, id
	post_type values are shown in the PostType schema, id is an i32
	*/
	pub filter: Option<String>,
	pub limit: Option<usize>,
	pub offset: Option<usize>,
}

#[utoipa::path(
	get,
	path = "/api/v1/posts",
	params(
		SearchParams
	),
	responses(
		(status = 200, body = Vec<Post>, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn search_posts(
	Query(query): Query<SearchParams>,
	State(state): State<AppState>,
) -> Result<Json<Vec<Post>>, (StatusCode, String)> {
	let index = state.meilisearch.index("posts");
	let mut search = meilisearch_sdk::search::SearchQuery::new(&index);

	search.query = query.query.as_ref().map(|query| query.as_str());

	let filter = if let Some(filter) = &query.filter {
		format!("({filter}) AND private=false")
	} else {
		String::from("private=false")
	};

	search.filter = Some(meilisearch_sdk::search::Filter::new(sqlx::Either::Left(
		filter.as_str(),
	)));

	search.limit = query.limit;
	search.offset = query.offset;

	let mut sort = vec![];
	if let Some(qsort) = &query.sort {
		sort.push(qsort.as_str());
	} else {
		sort.push("time:desc");
	}
	search.sort = Some(&sort);

	let posts = search
		.execute::<Post>()
		.await
		.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

	let posts = posts
		.hits
		.into_iter()
		.map(|p| p.result.id)
		.collect::<Vec<_>>();

	let mut vec = Vec::with_capacity(posts.len());
	for id in posts {
		if let Some(mut post) = Post::get_full(id, &state.db).await {
			for i in 0..post.files.len() {
				post.files[i] = format!(
					"https://divamodarchive.com/api/v1/posts/{}/download/{i}",
					post.id
				);
				post.local_files[i] = post.local_files[i]
					.split("/")
					.last()
					.map(|s| String::from(s))
					.unwrap_or(String::new());
			}
			vec.push(post);
		} else {
			_ = index.delete_document(id).await;
		}
	}

	Ok(Json(vec))
}

#[utoipa::path(
	get,
	path = "/api/v1/posts/count",
	params(
		SearchParams
	),
	responses(
		(status = 200, body = usize, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn count_posts(
	Query(query): Query<SearchParams>,
	State(state): State<AppState>,
) -> Result<Json<usize>, (StatusCode, String)> {
	let index = state.meilisearch.index("posts");
	let mut search = meilisearch_sdk::search::SearchQuery::new(&index);

	search.query = query.query.as_ref().map(|query| query.as_str());

	let filter = if let Some(filter) = &query.filter {
		format!("({filter}) AND private=false")
	} else {
		String::from("private=false")
	};

	search.filter = Some(meilisearch_sdk::search::Filter::new(sqlx::Either::Left(
		filter.as_str(),
	)));

	search.limit = query.limit;
	search.offset = query.offset;

	let mut sort = vec![];
	if let Some(qsort) = &query.sort {
		sort.push(qsort.as_str());
	} else {
		sort.push("time:desc");
	}
	search.sort = Some(&sort);

	let posts = search
		.execute::<Post>()
		.await
		.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

	Ok(Json(posts.estimated_total_hits.unwrap_or(0)))
}

pub async fn delete_post(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
) -> Result<(), StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	if !post.authors.iter().any(|u| u.id == user.id) && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}

	for file in post.local_files {
		_ = tokio::process::Command::new("rclone")
			.arg("delete")
			.arg(format!("pixeldrainfs:/divamodarchive/{}", file))
			.arg("--config=/etc/rclone-mnt.conf")
			.output()
			.await;
	}

	_ = sqlx::query!("DELETE FROM posts WHERE id = $1", post.id)
		.execute(&state.db)
		.await;

	_ = state
		.meilisearch
		.index("posts")
		.delete_document(post.id)
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("pvs"))
		.with_filter(&format!("post={}", post.id))
		.execute::<MeilisearchPv>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("modules"))
		.with_filter(&format!("post_id={}", post.id))
		.execute::<MeilisearchModule>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("cstm_items"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchCstmItem>()
	.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("nc_songs"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchNcSong>()
	.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("sprite_sets"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchDbEntry>()
	.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("sprites"))
		.with_filter(&format!("post_id={}", post.id))
		.execute::<MeilisearchDbEntry>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("aet_sets"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchDbEntry>()
	.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("aet_scenes"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchDbEntry>()
	.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("objsets"))
		.with_filter(&format!("post_id={}", post.id))
		.execute::<MeilisearchDbEntry>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("textures"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<MeilisearchDbEntry>()
	.await;

	Ok(())
}

pub async fn add_author(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(new_author): Json<String>,
) -> Result<Json<User>, StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}
	if post.authors.iter().any(|u| u.name == new_author) {
		return Err(StatusCode::BAD_REQUEST);
	}

	let new_author = sqlx::query_as!(User, "SELECT * FROM users WHERE name = $1", new_author)
		.fetch_one(&state.db)
		.await
		.map_err(|_| StatusCode::NOT_FOUND)?;

	_ = sqlx::query!(
		"INSERT INTO post_authors (post_id, user_id) VALUES ($1, $2)",
		post.id,
		new_author.id
	)
	.execute(&state.db)
	.await;

	Ok(Json(new_author))
}

pub async fn remove_author(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(removed_author): Json<String>,
) -> Result<(), StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}
	if !post.authors.iter().any(|u| u.name == removed_author) || user.name == removed_author {
		return Err(StatusCode::BAD_REQUEST);
	}

	let removed_author =
		sqlx::query_as!(User, "SELECT * FROM users WHERE name = $1", removed_author)
			.fetch_one(&state.db)
			.await
			.map_err(|_| StatusCode::NOT_FOUND)?;

	_ = sqlx::query!(
		"DELETE FROM post_authors WHERE post_id=$1 AND user_id=$2",
		post.id,
		removed_author.id
	)
	.execute(&state.db)
	.await;

	Ok(())
}

pub async fn add_dependency(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(dependency): Json<i32>,
) -> Result<Json<Post>, StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	let Some(dependency) = Post::get_short(dependency, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}

	if sqlx::query!(
		"SELECT FROM post_dependencies WHERE post_id = $1 AND dependency_id = $2",
		post.id,
		dependency.id
	)
	.fetch_optional(&state.db)
	.await
	.map_or(true, |opt| opt.is_some())
	{
		return Err(StatusCode::BAD_REQUEST);
	}

	if sqlx::query!(
		"SELECT FROM post_dependencies WHERE post_id = $1 AND dependency_id = $2",
		dependency.id,
		post.id
	)
	.fetch_optional(&state.db)
	.await
	.map_or(true, |opt| opt.is_some())
	{
		return Err(StatusCode::BAD_REQUEST);
	}

	_ = sqlx::query!(
		"INSERT INTO post_dependencies (post_id, dependency_id) VALUES ($1, $2)",
		post.id,
		dependency.id
	)
	.execute(&state.db)
	.await;

	Ok(Json(dependency))
}

#[derive(Serialize, Deserialize)]
pub struct DependencyDescription {
	pub id: i32,
	pub description: String,
}

pub async fn set_dependency_description(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(description): Json<DependencyDescription>,
) -> Result<(), StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	let Some(dependency) = Post::get_short(description.id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}

	_ = sqlx::query!(
		"UPDATE post_dependencies SET description=$1 WHERE post_id=$2 AND dependency_id=$3",
		description.description,
		post.id,
		dependency.id
	)
	.execute(&state.db)
	.await;

	Ok(())
}

pub async fn remove_dependency(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(dependency): Json<i32>,
) -> Result<(), StatusCode> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	let Some(dependency) = Post::get_short(dependency, &state.db).await else {
		return Err(StatusCode::NOT_FOUND);
	};

	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}

	_ = sqlx::query!(
		"DELETE FROM post_dependencies WHERE post_id=$1 AND dependency_id=$2",
		post.id,
		dependency.id
	)
	.execute(&state.db)
	.await;

	Ok(())
}

pub async fn report(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(complaint): Json<String>,
) {
	let now = time::OffsetDateTime::now_utc();
	let time = time::PrimitiveDateTime::new(now.date(), now.time());

	_ = sqlx::query!(
		"INSERT INTO reports (post_id, user_id, text, time) VALUES ($1, $2, $3, $4)",
		id,
		user.id,
		complaint,
		time
	)
	.execute(&state.db)
	.await;
}

#[derive(Serialize, Deserialize)]
pub struct CommentRequest {
	text: String,
	parent: Option<i32>,
}

pub async fn comment(
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(comment): Json<CommentRequest>,
) -> Result<(), StatusCode> {
	if Post::get_short(id, &state.db).await.is_none() {
		return Err(StatusCode::NOT_FOUND);
	}
	let now = time::OffsetDateTime::now_utc();
	let time = time::PrimitiveDateTime::new(now.date(), now.time());

	_ = sqlx::query!(
		"INSERT INTO post_comments (post_id, user_id, text, parent, time) VALUES ($1, $2, $3, $4, $5)",
		id,
		user.id,
		comment.text,
		comment.parent,
		time
	)
	.execute(&state.db)
	.await;

	Ok(())
}

pub async fn delete_comment(
	Path((post, comment)): Path<(i32, i32)>,
	user: User,
	State(state): State<AppState>,
) -> Result<(), StatusCode> {
	if Post::get_short(post, &state.db).await.is_none() {
		return Err(StatusCode::NOT_FOUND);
	}

	let comment_user = sqlx::query!(
		"SELECT user_id from post_comments WHERE id = $1 AND post_id = $2",
		comment,
		post
	)
	.fetch_one(&state.db)
	.await
	.map_err(|_| StatusCode::NOT_FOUND)?;

	if user.id != comment_user.user_id && !user.is_admin(&state.config) {
		return Err(StatusCode::UNAUTHORIZED);
	}

	_ = sqlx::query!(
		"DELETE FROM post_comments WHERE id = $1 AND post_id = $2",
		comment,
		post,
	)
	.execute(&state.db)
	.await;

	Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct UserSettings {
	display_name: String,
	public_likes: bool,
	theme: i32,
	show_explicit: bool,
}

pub async fn user_settings(
	user: User,
	State(state): State<AppState>,
	Json(settings): Json<UserSettings>,
) -> axum::http::HeaderMap {
	_ = sqlx::query!(
		"UPDATE users SET display_name = $2, public_likes = $3, theme = $4, show_explicit = $5 WHERE id = $1",
		user.id,
		settings.display_name,
		settings.public_likes,
		settings.theme,
		settings.show_explicit
	)
	.execute(&state.db)
	.await;

	let mut headers = axum::http::HeaderMap::new();
	headers.insert(
		"Clear-Site-Data",
		axum::http::HeaderValue::from_str("\"cache\"").unwrap(),
	);

	headers
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct PostDetail {
	pub post: Post,
	pub pvs: PvSearch,
	pub modules: ModuleSearch,
	pub cstm_items: CstmItemSearch,
	pub nc_songs: NcSongSearch,
	pub sprite_sets: BTreeMap<u32, String>,
	pub sprites: BTreeMap<u32, String>,
	pub aet_sets: BTreeMap<u32, String>,
	pub aet_scenes: BTreeMap<u32, String>,
	pub objsets: BTreeMap<u32, String>,
	pub textures: BTreeMap<u32, String>,
	pub pv_easy_count: usize,
	pub pv_normal_count: usize,
	pub pv_hard_count: usize,
	pub pv_extreme_count: usize,
	pub pv_exextreme_count: usize,
	pub conflicting_pvs: BTreeMap<i32, Vec<Pv>>,
	pub conflicting_modules: BTreeMap<i32, Vec<Module>>,
	pub conflicting_cstm_items: BTreeMap<i32, Vec<CstmItem>>,
	pub conflicting_pv_reservations: BTreeMap<i64, BTreeMap<i32, String>>,
	pub conflicting_module_reservations: BTreeMap<i64, BTreeMap<i32, String>>,
	pub conflicting_costume_reservations:
		BTreeMap<module_db::Chara, BTreeMap<i64, BTreeMap<i32, String>>>,
	pub conflicting_cstm_item_reservations: BTreeMap<i64, BTreeMap<i32, String>>,
	pub conflicting_sprite_sets: BTreeMap<i32, BTreeMap<u32, String>>,
	pub conflicting_sprites: BTreeMap<i32, BTreeMap<u32, String>>,
	pub conflicting_aet_sets: BTreeMap<i32, BTreeMap<u32, String>>,
	pub conflicting_aet_scenes: BTreeMap<i32, BTreeMap<u32, String>>,
	pub conflicting_objsets: BTreeMap<i32, BTreeMap<u32, String>>,
	pub conflicting_textures: BTreeMap<i32, BTreeMap<u32, String>>,
	pub conflict_posts: BTreeMap<i32, Post>,
	pub conflict_users: BTreeMap<i64, User>,
	pub requires_expatch: bool,
	pub requires_nc: bool,
}

#[utoipa::path(
	get,
	path = "/api/v1/posts/{id}/detail",
	params(
		("id" = i32, Path)
	),
	responses(
		(status = 200, body = PostDetail, content_type = "application/json"),
		(status = 401, body = String),
		(status = 404, body = String),
		(status = 500, body = String)
	)
)]
pub async fn post_detail(
	axum::extract::Path(id): axum::extract::Path<i32>,
	user: Result<User, ErrorTemplate>,
	State(state): State<AppState>,
) -> Result<Json<PostDetail>, (StatusCode, String)> {
	let Some(mut post) = Post::get_full(id, &state.db).await else {
		return Err((StatusCode::NOT_FOUND, String::from("Does not exist")));
	};

	if post.private {
		if let Ok(user) = user {
			if !post.authors.contains(&user) && !state.config.admins.contains(&user.id) {
				return Err((StatusCode::UNAUTHORIZED, String::from("Private post")));
			}
		} else {
			return Err((StatusCode::UNAUTHORIZED, String::from("Private post")));
		}
	}

	for i in 0..post.files.len() {
		post.files[i] = format!(
			"https://divamodarchive.com/api/v1/posts/{}/download/{i}",
			post.id
		);
		post.local_files[i] = post.local_files[i]
			.split("/")
			.last()
			.map(|s| String::from(s))
			.unwrap_or(String::new());
	}

	let Json(pvs) = search_pvs(
		Query(crate::api::ids::SearchParams {
			query: None,
			filter: Some(format!("post={}", post.id)),
			limit: Some(u32::MAX as usize),
			offset: Some(0),
		}),
		State(state.clone()),
	)
	.await
	.unwrap_or_default();

	let Json(modules) = search_modules(
		Query(crate::api::ids::SearchParams {
			query: None,
			filter: Some(format!("post_id={}", post.id)),
			limit: Some(u32::MAX as usize),
			offset: Some(0),
		}),
		State(state.clone()),
	)
	.await
	.unwrap_or_default();

	let Json(cstm_items) = search_cstm_items(
		Query(crate::api::ids::SearchParams {
			query: None,
			filter: Some(format!("post_id={}", post.id)),
			limit: Some(u32::MAX as usize),
			offset: Some(0),
		}),
		State(state.clone()),
	)
	.await
	.unwrap_or_default();

	let Json(nc_songs) = search_nc_songs(
		Query(crate::api::ids::SearchParams {
			query: None,
			filter: Some(format!("post_id={}", post.id)),
			limit: Some(u32::MAX as usize),
			offset: Some(0),
		}),
		State(state.clone()),
	)
	.await
	.unwrap_or_default();

	let pv_easy_count = pvs.pvs.iter().filter(|pv| pv.levels[0].is_some()).count();
	let pv_normal_count = pvs.pvs.iter().filter(|pv| pv.levels[1].is_some()).count();
	let pv_hard_count = pvs.pvs.iter().filter(|pv| pv.levels[2].is_some()).count();
	let pv_extreme_count = pvs.pvs.iter().filter(|pv| pv.levels[3].is_some()).count();
	let pv_exextreme_count = pvs.pvs.iter().filter(|pv| pv.levels[4].is_some()).count();

	let mut conflicting_pvs: BTreeMap<i32, Vec<Pv>> = BTreeMap::new();
	let mut conflicting_modules: BTreeMap<i32, Vec<Module>> = BTreeMap::new();
	let mut conflicting_cstm_items: BTreeMap<i32, Vec<CstmItem>> = BTreeMap::new();
	let mut conflict_posts: BTreeMap<i32, Post> = BTreeMap::new();

	if pvs.pvs.len() > 0 {
		let filter = pvs
			.pvs
			.iter()
			.map(|pv| format!("(pv_id={} AND post!={})", pv.id, pv.post.unwrap_or(-1)))
			.intersperse(String::from(" OR "))
			.collect::<String>();

		if let Ok(search) =
			meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("pvs"))
				.with_limit(u32::MAX as usize)
				.with_filter(&filter)
				.execute::<MeilisearchPv>()
				.await
		{
			for pv in search.results {
				let post = if pv.post == -1 {
					None
				} else if let Some(post) = conflict_posts.get(&pv.post) {
					Some(post.id)
				} else if let Some(post) = Post::get_full(pv.post, &state.db).await {
					conflict_posts.insert(post.id, post.clone());
					Some(post.id)
				} else {
					None
				};

				let pv = Pv {
					uid: BASE64_STANDARD.encode(pv.uid.to_ne_bytes()),
					id: pv.pv_id,
					name: pv.song_name,
					name_en: pv.song_name_en,
					song_info: pv.song_info,
					song_info_en: pv.song_info_en,
					levels: pv.levels,
					post,
				};

				if let Some(conflict) = conflicting_pvs.get_mut(&pv.id) {
					conflict.push(pv);
				} else {
					conflicting_pvs.insert(pv.id, vec![pv]);
				}
			}
		};
	}

	if modules.modules.len() > 0 {
		let filter = modules
			.modules
			.iter()
			.map(|module| {
				let item_filter = if module.module.cos.items.len() > 0 {
					format!(
						"OR (chara={} AND ({}))",
						serde_json::to_string(&module.module.chara)
							.unwrap()
							.trim_matches('\"'),
						module
							.module
							.cos
							.items
							.iter()
							.map(|item| format!("cos.items.id={}", item.id,))
							.intersperse(String::from(" OR "))
							.collect::<String>()
					)
				} else {
					String::new()
				};
				format!(
					"module_id={} OR (chara={} AND cos.id={}) {}",
					module.id,
					serde_json::to_string(&module.module.chara)
						.unwrap()
						.trim_matches('\"'),
					module.module.cos.id,
					item_filter,
				)
			})
			.intersperse(String::from(" OR "))
			.collect::<String>();

		if let Ok(search) =
			meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("modules"))
				.with_limit(u32::MAX as usize)
				.with_filter(&format!("({filter}) AND post_id!={}", post.id))
				.execute::<MeilisearchModule>()
				.await
		{
			for module in search.results {
				let post = if module.post_id == -1 {
					None
				} else if let Some(post) = conflict_posts.get(&module.post_id) {
					Some(post.id)
				} else if let Some(post) = Post::get_full(module.post_id, &state.db).await {
					conflict_posts.insert(post.id, post.clone());
					Some(post.id)
				} else {
					None
				};

				let module = Module {
					uid: BASE64_STANDARD.encode(module.uid.to_ne_bytes()),
					post,
					id: module.module_id,
					module: module.module,
				};

				if let Some(conflict) = conflicting_modules.get_mut(&module.id) {
					conflict.push(module);
				} else {
					conflicting_modules.insert(module.id, vec![module]);
				}
			}
		};
	}

	if cstm_items.cstm_items.len() > 0 {
		let filter = cstm_items
			.cstm_items
			.iter()
			.map(|cstm_item| {
				format!(
					"(customize_item_id={} AND post_id!={})",
					cstm_item.id,
					cstm_item.post.unwrap_or(-1)
				)
			})
			.intersperse(String::from(" OR "))
			.collect::<String>();

		if let Ok(search) =
			meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("cstm_items"))
				.with_limit(u32::MAX as usize)
				.with_filter(&filter)
				.execute::<MeilisearchCstmItem>()
				.await
		{
			for cstm_item in search.results {
				let post = if cstm_item.post_id == -1 {
					None
				} else if let Some(post) = conflict_posts.get(&cstm_item.post_id) {
					Some(post.id)
				} else if let Some(post) = Post::get_full(cstm_item.post_id, &state.db).await {
					conflict_posts.insert(post.id, post.clone());
					Some(post.id)
				} else {
					None
				};

				let customize_item = if cstm_item.customize_item.bind_module == Some(-1) {
					module_db::CustomizeItem {
						bind_module: None,
						chara: cstm_item.customize_item.chara,
						part: cstm_item.customize_item.part,
						obj_id: cstm_item.customize_item.obj_id,
						name: cstm_item.customize_item.name,
						name_jp: cstm_item.customize_item.name_jp,
						name_en: cstm_item.customize_item.name_en,
						name_cn: cstm_item.customize_item.name_cn,
						name_fr: cstm_item.customize_item.name_fr,
						name_ge: cstm_item.customize_item.name_ge,
						name_it: cstm_item.customize_item.name_it,
						name_kr: cstm_item.customize_item.name_kr,
						name_sp: cstm_item.customize_item.name_sp,
						name_tw: cstm_item.customize_item.name_tw,
					}
				} else {
					cstm_item.customize_item
				};

				let cstm_item = CstmItem {
					uid: BASE64_STANDARD.encode(cstm_item.uid.to_ne_bytes()),
					post,
					id: cstm_item.customize_item_id,
					cstm_item: customize_item,
				};

				if let Some(conflict) = conflicting_cstm_items.get_mut(&cstm_item.id) {
					conflict.push(cstm_item);
				} else {
					conflicting_cstm_items.insert(cstm_item.id, vec![cstm_item]);
				}
			}
		};
	}

	let mut conflicting_pv_reservations: BTreeMap<i64, BTreeMap<i32, String>> = BTreeMap::new();
	let mut conflicting_module_reservations: BTreeMap<i64, BTreeMap<i32, String>> = BTreeMap::new();
	let mut conflicting_costume_reservations: BTreeMap<
		module_db::Chara,
		BTreeMap<i64, BTreeMap<i32, String>>,
	> = BTreeMap::new();
	let mut conflicting_cstm_item_reservations: BTreeMap<i64, BTreeMap<i32, String>> =
		BTreeMap::new();
	let mut conflict_users: BTreeMap<i64, User> = BTreeMap::new();

	for pv in &pvs.pvs {
		let users = sqlx::query_as!(
			User,
			r#"
			SELECT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme, u.show_explicit
			FROM reservations r
			LEFT JOIN users u ON r.user_id = u.id
			WHERE r.reservation_type = 0
			AND (
				r.range_start >= $1
				OR r.range_start + r.length > $1
			)
			AND r.range_start <= $1
			"#,
			pv.id,
		)
		.fetch_all(&state.db)
		.await
		.unwrap_or_default();
		for user in users {
			if post.authors.contains(&user) {
				continue;
			}

			let label = if let Ok(label) = sqlx::query!(
				"SELECT label FROM reservation_labels WHERE reservation_type = $1 AND id = $2 AND user_id = $3",
				ReservationType::Song as i32,
				pv.id,
				user.id
			)
			.fetch_one(&state.db)
			.await
			{
				label.label
			} else {
				String::new()
			};

			if !conflicting_pv_reservations.contains_key(&user.id) {
				conflicting_pv_reservations.insert(user.id, BTreeMap::new());
			}
			let Some(conflict) = conflicting_pv_reservations.get_mut(&user.id) else {
				continue;
			};
			conflict.insert(pv.id, label);

			if !conflict_users.contains_key(&user.id) {
				conflict_users.insert(user.id, user);
			}
		}
	}

	for module in &modules.modules {
		let users = sqlx::query_as!(
			User,
			r#"
			SELECT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme, u.show_explicit
			FROM reservations r
			LEFT JOIN users u ON r.user_id = u.id
			WHERE
			r.reservation_type = 1
			AND (
				r.range_start >= $1
				OR r.range_start + r.length > $1
			)
			AND r.range_start <= $1
			"#,
			module.id,
		)
		.fetch_all(&state.db)
		.await
		.unwrap_or_default();
		for user in users {
			if post.authors.contains(&user) {
				continue;
			}

			let label = if let Ok(label) = sqlx::query!(
				"SELECT label FROM reservation_labels WHERE reservation_type = $1 AND id = $2 AND user_id = $3",
				ReservationType::Module as i32,
				module.id,
				user.id
			)
			.fetch_one(&state.db)
			.await
			{
				label.label
			} else {
				String::new()
			};

			if !conflicting_module_reservations.contains_key(&user.id) {
				conflicting_module_reservations.insert(user.id, BTreeMap::new());
			}
			let Some(conflict) = conflicting_module_reservations.get_mut(&user.id) else {
				continue;
			};
			conflict.insert(module.id, label);

			if !conflict_users.contains_key(&user.id) {
				conflict_users.insert(user.id, user);
			}
		}

		let users = sqlx::query_as!(
			User,
			r#"
			SELECT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme, u.show_explicit
			FROM reservations r
			LEFT JOIN users u ON r.user_id = u.id
			WHERE
			r.reservation_type = $1
			AND (
				r.range_start >= $2
				OR r.range_start + r.length > $2
			)
			AND r.range_start <= $2
			"#,
			module.module.chara.clone() as i32 + 10,
			module.module.cos.id,
		)
		.fetch_all(&state.db)
		.await
		.unwrap_or_default();
		for user in users {
			if post.authors.contains(&user) {
				continue;
			}

			let label = if let Ok(label) = sqlx::query!(
				"SELECT label FROM reservation_labels WHERE reservation_type = $1 AND id = $2 AND user_id = $3",
				module.module.chara.clone() as i32 + 10,
				module.module.cos.id,
				user.id
			)
			.fetch_one(&state.db)
			.await
			{
				label.label
			} else {
				String::new()
			};

			if !conflicting_costume_reservations.contains_key(&module.module.chara) {
				conflicting_costume_reservations
					.insert(module.module.chara.clone(), BTreeMap::new());
			}
			let Some(reservations) = conflicting_costume_reservations.get_mut(&module.module.chara)
			else {
				continue;
			};

			if !reservations.contains_key(&user.id) {
				reservations.insert(user.id, BTreeMap::new());
			}
			let Some(conflict) = reservations.get_mut(&user.id) else {
				continue;
			};
			conflict.insert(module.module.cos.id, label);

			if !conflict_users.contains_key(&user.id) {
				conflict_users.insert(user.id, user);
			}
		}
	}

	for cstm_item in &cstm_items.cstm_items {
		let users = sqlx::query_as!(
			User,
			r#"
			SELECT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme, u.show_explicit
			FROM reservations r
			LEFT JOIN users u ON r.user_id = u.id
			WHERE r.reservation_type = 2
			AND (
				r.range_start >= $1
				OR r.range_start + r.length > $1
			)
			AND r.range_start <= $1
			"#,
			cstm_item.id,
		)
		.fetch_all(&state.db)
		.await
		.unwrap_or_default();
		for user in users {
			if post.authors.contains(&user) {
				continue;
			}

			let label = if let Ok(label) = sqlx::query!(
				"SELECT label FROM reservation_labels WHERE reservation_type = $1 AND id = $2 AND user_id = $3",
				ReservationType::CstmItem as i32,
				cstm_item.id,
				user.id
			)
			.fetch_one(&state.db)
			.await
			{
				label.label
			} else {
				String::new()
			};

			if !conflicting_cstm_item_reservations.contains_key(&user.id) {
				conflicting_cstm_item_reservations.insert(user.id, BTreeMap::new());
			}
			let Some(conflict) = conflicting_cstm_item_reservations.get_mut(&user.id) else {
				continue;
			};
			conflict.insert(cstm_item.id, label);

			if !conflict_users.contains_key(&user.id) {
				conflict_users.insert(user.id, user);
			}
		}
	}

	let sprite_sets =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("sprite_sets"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry.name))
					.collect::<BTreeMap<_, _>>()
			})
			.unwrap_or_default();

	let sprites =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("sprites"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry.name))
					.collect::<BTreeMap<_, _>>()
			})
			.unwrap_or_default();

	let aet_sets =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aet_sets"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry.name))
					.collect::<BTreeMap<_, _>>()
			})
			.unwrap_or_default();

	let aet_scenes =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aet_scenes"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry.name))
					.collect::<BTreeMap<_, _>>()
			})
			.unwrap_or_default();

	let objsets =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("objsets"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry.name))
					.collect::<BTreeMap<_, _>>()
			})
			.unwrap_or_default();

	let textures =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("textures"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry.name))
					.collect::<BTreeMap<_, _>>()
			})
			.unwrap_or_default();

	let mut conflicting_sprite_sets: BTreeMap<i32, BTreeMap<u32, String>> = BTreeMap::new();
	let mut conflicting_sprites: BTreeMap<i32, BTreeMap<u32, String>> = BTreeMap::new();
	let mut conflicting_aet_sets: BTreeMap<i32, BTreeMap<u32, String>> = BTreeMap::new();
	let mut conflicting_aet_scenes: BTreeMap<i32, BTreeMap<u32, String>> = BTreeMap::new();
	let mut conflicting_objsets: BTreeMap<i32, BTreeMap<u32, String>> = BTreeMap::new();
	let mut conflicting_textures: BTreeMap<i32, BTreeMap<u32, String>> = BTreeMap::new();

	let search = sprite_sets
		.iter()
		.map(|(id, entry)| format!("(id={} AND name!='{}')", id, entry))
		.intersperse(String::from(" OR "))
		.collect::<String>();

	if let Ok(conflicts) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("sprite_sets"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("({search}) AND post_id!={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
	{
		for conflict in conflicts.results {
			if !conflicting_sprite_sets.contains_key(&conflict.post_id) {
				conflicting_sprite_sets.insert(conflict.post_id, BTreeMap::new());

				if conflict.post_id != -1 && !conflict_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflict_posts.insert(post.id, post);
					}
				}
			}
			let Some(existing) = conflicting_sprite_sets.get_mut(&conflict.post_id) else {
				continue;
			};
			existing.insert(conflict.id, conflict.name);
		}
	}

	let search = sprites
		.iter()
		.map(|(id, entry)| format!("(id={} AND name!='{}')", id, entry))
		.intersperse(String::from(" OR "))
		.collect::<String>();

	if let Ok(conflicts) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("sprites"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("({search}) AND post_id!={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
	{
		for conflict in conflicts.results {
			if !conflicting_sprites.contains_key(&conflict.post_id) {
				conflicting_sprites.insert(conflict.post_id, BTreeMap::new());

				if conflict.post_id != -1 && !conflict_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflict_posts.insert(post.id, post);
					}
				}
			}
			let Some(existing) = conflicting_sprites.get_mut(&conflict.post_id) else {
				continue;
			};
			existing.insert(conflict.id, conflict.name);
		}
	}

	let search = aet_sets
		.iter()
		.map(|(id, entry)| format!("(id={} AND name!='{}')", id, entry))
		.intersperse(String::from(" OR "))
		.collect::<String>();

	if let Ok(conflicts) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aet_sets"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("({search}) AND post_id!={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
	{
		for conflict in conflicts.results {
			if !conflicting_aet_sets.contains_key(&conflict.post_id) {
				conflicting_aet_sets.insert(conflict.post_id, BTreeMap::new());

				if conflict.post_id != -1 && !conflict_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflict_posts.insert(post.id, post);
					}
				}
			}
			let Some(existing) = conflicting_aet_sets.get_mut(&conflict.post_id) else {
				continue;
			};
			existing.insert(conflict.id, conflict.name);
		}
	}

	let search = aet_scenes
		.iter()
		.map(|(id, entry)| format!("(id={} AND name!='{}')", id, entry))
		.intersperse(String::from(" OR "))
		.collect::<String>();

	if let Ok(conflicts) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aet_scenes"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("({search}) AND post_id!={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
	{
		for conflict in conflicts.results {
			if !conflicting_aet_scenes.contains_key(&conflict.post_id) {
				conflicting_aet_scenes.insert(conflict.post_id, BTreeMap::new());

				if conflict.post_id != -1 && !conflict_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflict_posts.insert(post.id, post);
					}
				}
			}
			let Some(existing) = conflicting_aet_scenes.get_mut(&conflict.post_id) else {
				continue;
			};
			existing.insert(conflict.id, conflict.name);
		}
	}

	let search = objsets
		.iter()
		.map(|(id, entry)| format!("(id={} AND name!='{}')", id, entry))
		.intersperse(String::from(" OR "))
		.collect::<String>();

	if let Ok(conflicts) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("objsets"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("({search}) AND post_id!={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
	{
		for conflict in conflicts.results {
			if !conflicting_objsets.contains_key(&conflict.post_id) {
				conflicting_objsets.insert(conflict.post_id, BTreeMap::new());

				if conflict.post_id != -1 && !conflict_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflict_posts.insert(post.id, post);
					}
				}
			}
			let Some(existing) = conflicting_objsets.get_mut(&conflict.post_id) else {
				continue;
			};
			existing.insert(conflict.id, conflict.name);
		}
	}

	let search = textures
		.iter()
		.map(|(id, entry)| format!("(id={} AND name!='{}')", id, entry))
		.intersperse(String::from(" OR "))
		.collect::<String>();

	if let Ok(conflicts) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("textures"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("({search}) AND post_id!={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
	{
		for conflict in conflicts.results {
			if !conflicting_textures.contains_key(&conflict.post_id) {
				conflicting_textures.insert(conflict.post_id, BTreeMap::new());

				if conflict.post_id != -1 && !conflict_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflict_posts.insert(post.id, post);
					}
				}
			}
			let Some(existing) = conflicting_textures.get_mut(&conflict.post_id) else {
				continue;
			};
			existing.insert(conflict.id, conflict.name);
		}
	}

	let requires_expatch = pvs
		.pvs
		.iter()
		.any(|pv| (pv.levels[3].is_some() || pv.levels[4].is_some()) && pv.levels[2].is_none());

	let requires_nc = nc_songs.nc_songs.iter().any(|nc_song| {
		nc_songs.pvs.get(&nc_song.pv_id).map_or(false, |pvs| {
			pvs.iter().all(|pv| pv.post.unwrap_or(-1) != post.id)
		}) || nc_song
			.difficulties
			.iter()
			.filter_map(|difficulty| difficulty.clone())
			.all(|difficulty| difficulty.arcade.is_none())
	});

	Ok(Json(PostDetail {
		post,
		pvs,
		modules,
		cstm_items,
		nc_songs,
		sprite_sets,
		sprites,
		aet_sets,
		aet_scenes,
		objsets,
		textures,
		pv_easy_count,
		pv_normal_count,
		pv_hard_count,
		pv_extreme_count,
		pv_exextreme_count,
		conflicting_pvs,
		conflicting_modules,
		conflicting_cstm_items,
		conflicting_pv_reservations,
		conflicting_module_reservations,
		conflicting_costume_reservations,
		conflicting_cstm_item_reservations,
		conflicting_sprite_sets,
		conflicting_sprites,
		conflicting_aet_sets,
		conflicting_aet_scenes,
		conflicting_objsets,
		conflicting_textures,
		conflict_posts,
		conflict_users,
		requires_expatch,
		requires_nc,
	}))
}
