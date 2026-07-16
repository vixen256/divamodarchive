use crate::models::*;
use crate::{AppState, Config};
use askama::Template;
use askama_web::WebTemplate;
use axum::{
	RequestPartsExt, Router,
	extract::*,
	http::{StatusCode, header::*},
	response::*,
	routing::*,
};
use axum_extra::extract::CookieJar;

pub fn route(state: AppState) -> Router {
	Router::new()
		.route("/", get(search))
		.route("/about", get(about))
		.route("/post/{id}", get(post_detail))
		.route("/posts/{id}", get(post_redirect))
		.route("/post/{id}/edit", get(edit))
		.route("/post/{id}/report", get(report))
		.route("/liked/{id}", get(liked))
		.route("/user/{id}", get(user))
		.route("/upload", get(upload))
		.route("/settings", get(settings))
		//.route("/admin", get(admin))
		.layer(axum::middleware::from_fn(axum_html_minifier::html_minifier))
		.with_state(state)
}

mod filters {
	#[askama::filter_fn]
	pub fn prettify_num<T: std::fmt::Display>(
		s: T,
		_: &dyn askama::Values,
	) -> askama::Result<String> {
		let num: u64 = match s.to_string().parse() {
			Ok(num) => num,
			Err(e) => return Err(askama::Error::Custom(Box::new(e))),
		};

		let suffixes = ["", "K", "M", "B"];
		let mut remainder = 0;
		let mut value = num;
		for suffix in suffixes {
			if value < 1000 {
				if remainder > 0 {
					return Ok(format!("{value}.{remainder}{suffix}"));
				} else {
					return Ok(format!("{value}{suffix}"));
				}
			}
			remainder = (value % 1000) / 100;
			value /= 1000;
		}

		Ok(format!("{num}"))
	}

	#[askama::filter_fn]
	pub fn prettify_num_byte<T: std::fmt::Display>(
		s: T,
		_: &dyn askama::Values,
	) -> askama::Result<String> {
		let num: u64 = match s.to_string().parse() {
			Ok(num) => num,
			Err(e) => return Err(askama::Error::Custom(Box::new(e))),
		};

		let suffixes = ["B", "KB", "MB", "GB"];
		let mut remainder = 0;
		let mut value = num;
		for suffix in suffixes {
			if value < 1024 {
				if remainder > 0 {
					return Ok(format!("{value}.{remainder}{suffix}"));
				} else {
					return Ok(format!("{value}{suffix}"));
				}
			}
			remainder = (value % 1024) / 100;
			value /= 1024;
		}

		Ok(format!("{num}"))
	}
}

#[derive(Clone)]
pub struct BaseTemplate {
	pub user: Option<User>,
	pub config: Config,
	pub jwt: Option<String>,
	pub report_count: Option<i64>,
	pub has_likes: bool,
	pub pending_upload: Option<Post>,
	pub uri: String,
}

impl<S> FromRequestParts<S> for BaseTemplate
where
	S: Send + Sync,
	AppState: FromRef<S>,
{
	type Rejection = std::convert::Infallible;

	async fn from_request_parts(
		parts: &mut axum::http::request::Parts,
		state: &S,
	) -> Result<Self, Self::Rejection> {
		let cookies = parts.extract::<CookieJar>().await.unwrap();
		let jwt = if let Some(cookie) = cookies.get(&AUTHORIZATION.to_string()) {
			Some(String::from(cookie.value()))
		} else {
			if let Some(auth) = parts.headers.get(AUTHORIZATION) {
				if let Ok(auth) = auth.to_str() {
					Some(String::from(auth.replace("Bearer ", "")))
				} else {
					None
				}
			} else {
				None
			}
		};

		let user = if jwt.is_some() {
			User::from_request_parts(parts, state).await.ok()
		} else {
			None
		};
		let state: AppState = AppState::from_ref(state);

		let report_count = if let Some(user) = &user {
			if user.is_admin(&state.config) {
				sqlx::query!("SELECT COUNT(*) FROM reports WHERE admin_handled IS NULL")
					.fetch_one(&state.db)
					.await
					.ok()
					.map(|count| count.count.unwrap_or(0))
			} else {
				None
			}
		} else {
			None
		};

		let has_likes = if let Some(user) = &user {
			sqlx::query!(
				"SELECT COUNT(*) FROM liked_posts WHERE user_id = $1",
				user.id
			)
			.fetch_one(&state.db)
			.await
			.map_or(false, |record| record.count.unwrap_or(0) > 0)
		} else {
			false
		};

		let pending_upload = if let Some(user) = &user {
			if let Ok(post_id) = sqlx::query!(
				"SELECT post_id FROM pending_uploads WHERE user_id = $1",
				user.id
			)
			.fetch_one(&state.db)
			.await
			{
				Post::get_short(post_id.post_id, &state.db).await
			} else {
				None
			}
		} else {
			None
		};

		let uri = parts.uri.path().to_string();

		Ok(Self {
			user,
			config: state.config,
			jwt,
			report_count,
			has_likes,
			pending_upload,
			uri,
		})
	}
}

impl BaseTemplate {
	pub fn theme(&self) -> Theme {
		self.user
			.as_ref()
			.map_or(Theme::default(), |user| user.theme)
	}

	pub fn show_explicit(&self) -> bool {
		self.user.as_ref().map_or(false, |user| user.show_explicit)
	}
}

#[derive(Template, WebTemplate)]
#[template(path = "about.html")]
struct AboutTemplate {
	base: BaseTemplate,
}

async fn about(base: BaseTemplate) -> AboutTemplate {
	AboutTemplate { base }
}

#[derive(Template, WebTemplate)]
#[template(path = "liked.html")]
struct LikedTemplate {
	base: BaseTemplate,
	posts: Vec<Post>,
	owner: User,
}

async fn liked(
	Path(id): Path<i64>,
	base: BaseTemplate,
	State(state): State<AppState>,
) -> Result<LikedTemplate, ErrorTemplate> {
	let Some(owner) = User::get(id, &state.db).await else {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::BAD_REQUEST,
		});
	};

	if !owner.public_likes && !base.user.as_ref().map_or(false, |user| user.id == owner.id) {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::UNAUTHORIZED,
		});
	}

	let liked_posts = sqlx::query!(
		r#"
		SELECT p.id
		FROM liked_posts lp
		LEFT JOIN posts p ON lp.post_id = p.id
		WHERE lp.user_id = $1 AND p.private = false
		ORDER by p.time DESC
		"#,
		id,
	)
	.fetch_all(&state.db)
	.await
	.map_err(|_| ErrorTemplate {
		base: base.clone(),
		status: StatusCode::INTERNAL_SERVER_ERROR,
	})?;

	let mut posts = Vec::new();
	for post in liked_posts {
		if let Some(post) = Post::get_short(post.id, &state.db).await {
			posts.push(post);
		}
	}

	Ok(LikedTemplate { base, posts, owner })
}

#[derive(Template, WebTemplate)]
#[template(path = "user.html")]
struct UserTemplate {
	base: BaseTemplate,
	posts: Vec<Post>,
	owner: User,
	total_likes: i64,
	total_downloads: i64,
	has_likes: bool,
}

async fn user(
	Path(id): Path<i64>,
	base: BaseTemplate,
	State(state): State<AppState>,
) -> Result<UserTemplate, ErrorTemplate> {
	let Some(owner) = User::get(id, &state.db).await else {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::BAD_REQUEST,
		});
	};

	let user_posts = sqlx::query!(
		r#"
		SELECT p.id
		FROM post_authors pa
		LEFT JOIN posts p ON pa.post_id = p.id
		WHERE pa.user_id = $1
		ORDER BY p.time DESC
		"#,
		id
	)
	.fetch_all(&state.db)
	.await
	.map_err(|_| ErrorTemplate {
		base: base.clone(),
		status: StatusCode::BAD_REQUEST,
	})?;

	let mut posts = Vec::new();
	for post in user_posts {
		if let Some(post) = Post::get_short(post.id, &state.db).await {
			if post.private {
				if !base.user.as_ref().map_or(false, |user| {
					post.authors.contains(user) || user.is_admin(&state.config)
				}) {
					continue;
				}
			}
			posts.push(post);
		}
	}

	let (total_likes, total_downloads) = posts.iter().fold((0, 0), |acc, post| {
		(acc.0 + post.like_count, acc.1 + post.download_count)
	});

	let has_likes = sqlx::query!("SELECT COUNT(*) FROM liked_posts WHERE user_id = $1", id)
		.fetch_one(&state.db)
		.await
		.map_or(false, |record| record.count.unwrap_or(0) > 0);

	Ok(UserTemplate {
		base,
		posts,
		owner,
		total_likes,
		total_downloads,
		has_likes,
	})
}

#[derive(Template, WebTemplate)]
#[template(path = "post.html")]
struct PostTemplate {
	base: BaseTemplate,
	user: Option<User>,
	jwt: Option<String>,
	has_liked: bool,
	is_author: bool,
	post: Post,
	config: Config,
	body_markdown: String,
}

async fn post_redirect(Path(id): Path<i32>) -> Redirect {
	Redirect::permanent(&format!("https://taikomodarchive.com/post/{id}"))
}

async fn post_detail(
	Path(id): Path<i32>,
	State(state): State<AppState>,
	base: BaseTemplate,
) -> Result<PostTemplate, ErrorTemplate> {
	let Json(post) = crate::api::posts::get_post(Path(id), State(state.clone()))
		.await
		.map_err(|status| ErrorTemplate {
			base: base.clone(),
			status,
		})?;

	let has_liked = if let Some(user) = &base.user {
		let Ok(has_liked) = sqlx::query!(
			"SELECT COUNT(*) FROM liked_posts WHERE post_id = $1 AND user_id = $2",
			post.id,
			user.id
		)
		.fetch_one(&state.db)
		.await
		else {
			return Err(ErrorTemplate {
				base,
				status: StatusCode::INTERNAL_SERVER_ERROR,
			});
		};

		has_liked.count.unwrap_or(0) > 0
	} else {
		false
	};

	let is_author = if let Some(user) = &base.user {
		post.authors.contains(user)
	} else {
		false
	};

	let options = comrak::Options {
		extension: comrak::ExtensionOptions::builder()
			.strikethrough(true)
			.table(true)
			.autolink(true)
			.tasklist(true)
			.superscript(true)
			.underline(true)
			.subscript(true)
			.spoiler(true)
			.build(),
		parse: comrak::ParseOptions::builder()
			.smart(true)
			.relaxed_autolinks(true)
			.build(),
		render: comrak::RenderOptions::builder().escape(true).build(),
	};

	let body_markdown = comrak::markdown_to_html(&post.text, &options);

	Ok(PostTemplate {
		user: base.user.clone(),
		jwt: base.jwt.clone(),
		has_liked,
		is_author,
		base,
		post,
		config: state.config,
		body_markdown,
	})
}

#[derive(Template, WebTemplate)]
#[template(path = "search.html")]
struct SearchTemplate {
	base: BaseTemplate,
	posts: Vec<Post>,
	query: Option<crate::api::posts::SearchParams>,
}

async fn search(
	Query(query): Query<crate::api::posts::SearchParams>,
	base: BaseTemplate,
	State(state): State<AppState>,
) -> Result<SearchTemplate, ErrorTemplate> {
	if query.query.is_some() || query.sort.is_some() || query.filter.is_some() {
		let Json(posts) =
			crate::api::posts::search_posts(Query(query.clone()), State(state.clone()))
				.await
				.map_err(|(status, _)| ErrorTemplate {
					base: base.clone(),
					status,
				})?;

		return Ok(SearchTemplate {
			base,
			posts,
			query: Some(query),
		});
	}

	let latest_posts = sqlx::query!(
		r#"
		SELECT id
		FROM posts
		WHERE private = false
		ORDER BY time DESC
		LIMIT 40
		"#
	)
	.fetch_all(&state.db)
	.await
	.map_err(|_| ErrorTemplate {
		base: base.clone(),
		status: StatusCode::INTERNAL_SERVER_ERROR,
	})?;

	let mut posts = Vec::new();
	for post in latest_posts {
		if let Some(post) = Post::get_short(post.id, &state.db).await {
			posts.push(post);
		}
	}

	Ok(SearchTemplate {
		base,
		posts,
		query: None,
	})
}

#[derive(Template, WebTemplate)]
#[template(path = "settings.html")]
struct SettingsTemplate {
	base: BaseTemplate,
	user: User,
}

async fn settings(base: BaseTemplate, user: User) -> SettingsTemplate {
	SettingsTemplate { base, user }
}

#[derive(Template, WebTemplate)]
#[template(path = "report.html")]
struct ReportTemplate {
	base: BaseTemplate,
	post: Post,
}

async fn report(
	Path(id): Path<i32>,
	base: BaseTemplate,
	_: User,
	State(state): State<AppState>,
) -> Result<ReportTemplate, ErrorTemplate> {
	let Some(post) = Post::get_short(id, &state.db).await else {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::NOT_FOUND,
		});
	};
	if post.private {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::UNAUTHORIZED,
		});
	}

	Ok(ReportTemplate { base, post })
}

#[derive(Template, WebTemplate)]
#[template(path = "edit.html")]
struct EditTemplate {
	base: BaseTemplate,
	post: Post,
	files: Vec<String>,
	completed: Vec<i64>,
	length: Vec<i64>,
}

async fn edit(
	base: BaseTemplate,
	Path(id): Path<i32>,
	user: User,
	State(state): State<AppState>,
) -> Result<EditTemplate, ErrorTemplate> {
	let base = BaseTemplate {
		user: base.user,
		config: base.config,
		jwt: base.jwt,
		report_count: base.report_count,
		has_likes: base.has_likes,
		pending_upload: None,
		uri: base.uri,
	};

	let Some(post) = Post::get_full(id, &state.db).await else {
		return Err(ErrorTemplate {
			base: base.clone(),
			status: StatusCode::NOT_FOUND,
		});
	};
	if !post.authors.contains(&user) && !user.is_admin(&state.config) {
		return Err(ErrorTemplate {
			base: base.clone(),
			status: StatusCode::UNAUTHORIZED,
		});
	}

	if sqlx::query!(
		"SELECT files FROM pending_uploads WHERE post_id = $1 AND user_id != $2",
		id,
		user.id
	)
	.fetch_one(&state.db)
	.await
	.is_ok()
	{
		return Err(ErrorTemplate {
			base: base.clone(),
			status: StatusCode::UNAUTHORIZED,
		});
	}

	if sqlx::query!(
		"SELECT files FROM pending_uploads WHERE post_id != $1 AND user_id = $2",
		id,
		user.id
	)
	.fetch_one(&state.db)
	.await
	.is_ok()
	{
		return Err(ErrorTemplate {
			base: base.clone(),
			status: StatusCode::CONFLICT,
		});
	}

	let (files, completed, length) = if let Ok(pending_upload) = sqlx::query!(
		"SELECT files, completed, length FROM pending_uploads WHERE post_id = $1 AND user_id = $2",
		post.id,
		user.id
	)
	.fetch_one(&state.db)
	.await
	{
		(
			pending_upload.files,
			pending_upload.completed,
			pending_upload.length,
		)
	} else {
		(Vec::new(), Vec::new(), Vec::new())
	};

	if files.len() != completed.len() || completed.len() != length.len() {
		return Err(ErrorTemplate {
			base: base.clone(),
			status: StatusCode::INTERNAL_SERVER_ERROR,
		});
	}

	Ok(EditTemplate {
		base,
		post,
		files,
		completed,
		length,
	})
}

#[derive(Template, WebTemplate)]
#[template(path = "upload.html")]
struct UploadTemplate {
	base: BaseTemplate,
}

async fn upload(
	base: BaseTemplate,
	user: User,
	State(state): State<AppState>,
) -> Result<UploadTemplate, ErrorTemplate> {
	if sqlx::query!(
		"SELECT files FROM pending_uploads WHERE user_id = $1",
		user.id
	)
	.fetch_one(&state.db)
	.await
	.is_ok()
	{
		return Err(ErrorTemplate {
			base: base.clone(),
			status: StatusCode::CONFLICT,
		});
	}

	Ok(UploadTemplate { base })
}
