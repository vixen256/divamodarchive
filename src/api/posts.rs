use crate::AppState;
use crate::models::*;
use axum::{
	extract::*,
	http::{StatusCode, header},
	response::*,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

#[derive(Serialize, Deserialize)]
struct CloudflareDirectUploadResult {
	id: String,
	#[serde(rename = "uploadURL")]
	upload_url: String,
}

#[derive(Serialize, Deserialize)]
struct CloudflareDirectUpload {
	success: bool,
	result: CloudflareDirectUploadResult,
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
	let response = response.json::<CloudflareDirectUpload>().await;
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

#[derive(Serialize, Deserialize)]
pub struct PostUploadData {
	pub id: Option<i32>,
	pub name: String,
	pub text: String,
	pub post_type: i32,
	pub filenames: Option<Vec<String>>,
	pub image: Option<String>,
	pub images_extra: Option<Vec<String>>,
}

pub async fn edit(
	user: User,
	State(state): State<AppState>,
	Json(post): Json<PostUploadData>,
) -> Result<(), StatusCode> {
	let Some(post_id) = post.id else {
		return Err(StatusCode::BAD_REQUEST);
	};
	let authors = sqlx::query!(
		"SELECT user_id FROM post_authors WHERE post_id = $1",
		post_id
	)
	.fetch_all(&state.db)
	.await
	.map_err(|_| StatusCode::BAD_REQUEST)?;
	if !authors.iter().any(|u| u.user_id == user.id) {
		return Err(StatusCode::BAD_REQUEST)?;
	}

	sqlx::query!(
		"UPDATE posts SET name = $2, text = $3, type = $4 WHERE id = $1",
		post_id,
		post.name,
		post.text,
		post.post_type,
	)
	.execute(&state.db)
	.await
	.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

	if let Some(post) = Post::get_short(post_id, &state.db).await {
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

pub async fn upload_ws(ws: ws::WebSocketUpgrade, State(state): State<AppState>) -> Response {
	ws.on_upgrade(move |socket| real_upload_ws_wrapper(socket, state))
}

pub async fn real_upload_ws_wrapper(socket: ws::WebSocket, state: AppState) {
	crate::ACTIVE_CONNECTIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
	real_upload_ws(socket, state).await;
	crate::ACTIVE_CONNECTIONS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
}

pub async fn real_upload_ws(mut socket: ws::WebSocket, state: AppState) {
	let Some(Ok(message)) = socket.recv().await else {
		return;
	};

	let ws::Message::Text(message) = message else {
		return;
	};

	let Ok(user) = User::parse(&message, &state).await else {
		return;
	};

	let Some(Ok(message)) = socket.recv().await else {
		return;
	};

	let ws::Message::Text(message) = message else {
		return;
	};

	let Ok(params) = serde_json::from_str::<PostUploadData>(&message) else {
		return;
	};

	let Some(filenames) = params.filenames else {
		return;
	};
	let Some(image) = params.image else {
		return;
	};
	if let Some(post_id) = params.id {
		let Ok(authors) = sqlx::query!(
			"SELECT user_id FROM post_authors WHERE post_id = $1",
			post_id
		)
		.fetch_all(&state.db)
		.await
		else {
			return;
		};
		if !authors.iter().any(|u| u.user_id == user.id) {
			return;
		}
	}

	if !image.starts_with("https://divamodarchive.com/cdn-cgi/imagedelivery")
		|| reqwest::get(&image).await.is_err()
	{
		return;
	}
	if let Some(extra_images) = &params.images_extra {
		for image in extra_images {
			if !image.starts_with("https://divamodarchive.com/cdn-cgi/imagedelivery")
				|| reqwest::get(image).await.is_err()
			{
				return;
			}
		}
	}

	let mut filepaths = Vec::new();
	for filename in &filenames {
		let filepath = format!("{}/{}", user.id, filename);
		_ = tokio::fs::create_dir(format!("/pixeldrain/{}", user.id)).await;
		let Ok(mut file) = tokio::fs::File::create(&format!("/pixeldrain/{}", &filepath)).await
		else {
			return;
		};
		_ = socket.send(ws::Message::Text("Ready".into())).await;

		while let Some(message) = socket.recv().await {
			let Ok(message) = message else {
				return;
			};

			if let ws::Message::Binary(chunk) = message {
				let Ok(_) = file.write_all(&chunk).await else {
					return;
				};
				let Ok(_) = file.sync_data().await else {
					return;
				};
				_ = socket.send(ws::Message::Text("Ready".into())).await;
			} else if let ws::Message::Close(_) = message {
				return;
			} else {
				break;
			}
		}

		_ = file.sync_all().await;

		filepaths.push(filepath);
	}

	let mut images = Vec::new();
	images.push(image);
	if let Some(extra_images) = params.images_extra {
		for image in extra_images {
			images.push(image);
		}
	}

	let now = time::OffsetDateTime::now_utc();
	let time = time::PrimitiveDateTime::new(now.date(), now.time());

	let mut downloads = Vec::new();

	for filepath in &filepaths {
		let download = get_download_link(filepath).await;
		let Some(download) = download else {
			eprintln!("Failed to get public link for {filepath}");
			return;
		};
		downloads.push(download);
	}

	let post_id = if let Some(post_id) = params.id {
		let Some(post) = Post::get_full(post_id, &state.db).await else {
			return;
		};

		_ = sqlx::query!(
				"UPDATE posts SET name = $2, text = $3, type = $4, files = $5, images = $6, time = $7, local_files = $8 WHERE id = $1",
				post_id,
				params.name,
				params.text,
				params.post_type,
				&downloads,
				&images,
				time,
				&filepaths,
			)
			.execute(&state.db)
			.await;

		let pvs = state.meilisearch.index("pvs");
		_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&pvs)
			.with_filter(&format!("post={}", post_id))
			.execute::<crate::api::ids::MeilisearchPv>()
			.await;

		let modules = state.meilisearch.index("modules");
		_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&modules)
			.with_filter(&format!("post_id={}", post_id))
			.execute::<crate::api::ids::MeilisearchModule>()
			.await;

		let cstm_items = state.meilisearch.index("cstm_items");
		_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&cstm_items)
			.with_filter(&format!("post_id={}", post_id))
			.execute::<crate::api::ids::MeilisearchCstmItem>()
			.await;

		for file in post.local_files {
			if !filepaths.contains(&file) {
				_ = tokio::process::Command::new("rclone")
					.arg("delete")
					.arg(format!("pixeldrainfs:/divamodarchive/{}", file))
					.arg("--config=/etc/rclone-mnt.conf")
					.output()
					.await;
			}
		}

		post_id
	} else {
		let Ok(id) = sqlx::query!("INSERT INTO posts (name, text, images, files, time, type, local_files) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING ID", params.name, params.text, &images, &downloads, time, params.post_type, &filepaths)
				.fetch_one(&state.db)
				.await else {
					return;
				};

		_ = sqlx::query!(
			"INSERT INTO post_authors (post_id, user_id) VALUES ($1, $2)",
			id.id,
			user.id,
		)
		.execute(&state.db)
		.await;

		id.id
	};

	if let Some(post) = Post::get_short(post_id, &state.db).await {
		_ = state
			.meilisearch
			.index("posts")
			.add_or_update(&[post], None)
			.await;
	};

	tokio::spawn(crate::api::ids::extract_post_data(post_id, state.clone()));

	_ = socket
		.send(ws::Message::Text(format!("/post/{post_id}").into()))
		.await;
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
		format!("{filter}")
	} else {
		String::new()
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
		format!("{filter}")
	} else {
		String::new()
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

	if !post.authors.iter().any(|u| u.id == user.id) {
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

	if !post.authors.iter().any(|u| u.id == user.id) {
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
