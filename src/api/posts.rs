use crate::AppState;
use crate::models::*;
use axum::{
	extract::*,
	http::{StatusCode, header},
	response::*,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

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
	if data.private && post.files.is_empty() {
		return Err(StatusCode::BAD_REQUEST);
	}

	sqlx::query!(
		"UPDATE posts SET name = $2, text = $3, type = $4, private = $5 WHERE id = $1",
		id,
		data.name,
		data.text,
		data.post_type,
		data.private
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
		.execute::<crate::api::ids::MeilisearchPv>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&state.meilisearch.index("modules"))
		.with_filter(&format!("post_id={}", post.id))
		.execute::<crate::api::ids::MeilisearchModule>()
		.await;

	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(
		&state.meilisearch.index("cstm_items"),
	)
	.with_filter(&format!("post_id={}", post.id))
	.execute::<crate::api::ids::MeilisearchCstmItem>()
	.await;

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
			.arg("move")
			.arg(format!("/pixeldrain/{}/pending/{}", user.id, file))
			.arg(format!("/pixeldrain/{}/{}", user.id, file))
			.arg("--config=/etc/rclone-mnt.conf")
			.output()
			.await;
	}

	let files = pending_upload
		.files
		.iter()
		.map(|file| format!("{}/{file}", user.id))
		.collect::<Vec<_>>();

	let mut downloads = Vec::new();
	for file in &files {
		for i in 0..5 {
			let Some(download) = get_download_link(file).await else {
				if i == 4 {
					_ = socket
						.send(ws::Message::Text(ws::Utf8Bytes::from_static(
							"{\"error\": \"Failed to get public download link\"}",
						)))
						.await;
					return;
				}
				tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
				continue;
			};
			if download.is_empty() {
				if i == 4 {
					_ = socket
						.send(ws::Message::Text(ws::Utf8Bytes::from_static(
							"{\"error\": \"Failed to get public download link\"}",
						)))
						.await;
					return;
				}
				tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
				continue;
			}
			downloads.push(download);
			break;
		}
	}

	let now = time::OffsetDateTime::now_utc();
	let time = time::PrimitiveDateTime::new(now.date(), now.time());

	_ = sqlx::query!(
		"UPDATE posts SET files = $1, local_files = $2, time = $3 WHERE id = $4",
		&downloads,
		&files,
		time,
		post.id,
	)
	.execute(&state.db)
	.await;

	_ = sqlx::query!("DELETE FROM pending_uploads WHERE user_id = $1", user.id)
		.execute(&state.db)
		.await;

	tokio::spawn(crate::api::ids::extract_post_data(post.id, state.clone()));

	_ = socket
		.send(ws::Message::Text(ws::Utf8Bytes::from_static(
			"{\"success\": \"\"}",
		)))
		.await;
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

	tokio::spawn(crate::api::ids::extract_post_data(id, state.clone()));

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

#[derive(Serialize, Deserialize, Clone)]
pub struct SearchParams {
	pub query: Option<String>,
	pub sort: Option<String>,
	pub filter: Option<String>,
	pub limit: Option<usize>,
	pub offset: Option<usize>,
}

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
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

	let pvs = state.meilisearch.index("pvs");
	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&pvs)
		.with_filter(&format!("post={}", post.id))
		.execute::<crate::api::ids::MeilisearchPv>()
		.await;

	let modules = state.meilisearch.index("modules");
	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&modules)
		.with_filter(&format!("post_id={}", post.id))
		.execute::<crate::api::ids::MeilisearchModule>()
		.await;

	let cstm_items = state.meilisearch.index("cstm_items");
	_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&cstm_items)
		.with_filter(&format!("post_id={}", post.id))
		.execute::<crate::api::ids::MeilisearchCstmItem>()
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
}

pub async fn user_settings(
	user: User,
	State(state): State<AppState>,
	Json(settings): Json<UserSettings>,
) {
	_ = sqlx::query!(
		"UPDATE users SET display_name = $1, public_likes = $2, theme = $3 WHERE id = $4",
		settings.display_name,
		settings.public_likes,
		settings.theme,
		user.id
	)
	.execute(&state.db)
	.await;
}
