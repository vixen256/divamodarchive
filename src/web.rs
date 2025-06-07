use crate::api::ids::*;
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
use itertools::*;
use std::collections::*;

pub fn route(state: AppState) -> Router {
	Router::new()
		.route("/", get(search))
		.route("/about", get(about))
		.route("/post/{id}", get(post_detail))
		.route("/posts/{id}", get(post_redirect))
		.route("/post/{id}/edit", get(upload))
		.route("/post/{id}/report", get(report))
		.route("/liked/{id}", get(liked))
		.route("/user/{id}", get(user))
		.route("/reservations/{id}", get(user_reservations))
		.route("/upload", get(upload))
		.route("/settings", get(settings))
		.route("/pvs", get(pvs))
		.route("/modules", get(modules))
		.route("/cstm_items", get(cstm_items))
		.route("/pv_spreadsheet", get(pv_spreadsheet))
		.route("/reserve", get(reserve))
		//.route("/admin", get(admin))
		.layer(axum::middleware::from_fn(axum_html_minifier::html_minifier))
		.with_state(state)
}

mod filters {
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
}

#[derive(Clone)]
pub struct BaseTemplate {
	pub user: Option<User>,
	pub config: Config,
	pub jwt: Option<String>,
	pub report_count: Option<i64>,
	pub has_reservations: bool,
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

		let has_reservations = if let Some(user) = &user {
			sqlx::query!(
				"SELECT COUNT(*) FROM reservations WHERE user_id = $1",
				user.id
			)
			.fetch_one(&state.db)
			.await
			.map_or(false, |record| record.count.unwrap_or(0) > 0)
		} else {
			false
		};

		Ok(Self {
			user,
			config: state.config,
			jwt,
			report_count,
			has_reservations,
		})
	}
}

impl BaseTemplate {
	pub fn theme(&self) -> Theme {
		let Some(user) = &self.user else {
			return Theme::default();
		};
		user.theme
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
		WHERE lp.user_id = $1
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
	has_reservations: bool,
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
			posts.push(post);
		}
	}

	let (total_likes, total_downloads) = posts.iter().fold((0, 0), |acc, post| {
		(acc.0 + post.like_count, acc.1 + post.download_count)
	});

	let reservation_count =
		sqlx::query!("SELECT COUNT(*) FROM reservations WHERE user_id = $1", id)
			.fetch_one(&state.db)
			.await
			.map_or(0, |record| record.count.unwrap_or(0));

	Ok(UserTemplate {
		base,
		posts,
		owner,
		total_likes,
		total_downloads,
		has_reservations: reservation_count > 0,
	})
}

#[derive(Template, WebTemplate)]
#[template(path = "user_reservations.html")]
struct UserReservationsTemplate {
	base: BaseTemplate,
	owner: User,
	song_reservations: BTreeMap<i32, Reservation>,
	module_reservations: BTreeMap<i32, Reservation>,
	cstm_item_reservations: BTreeMap<i32, Reservation>,
}

async fn user_reservations(
	Path(id): Path<i64>,
	base: BaseTemplate,
	State(state): State<AppState>,
) -> Result<UserReservationsTemplate, ErrorTemplate> {
	let Some(owner) = User::get(id, &state.db).await else {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::BAD_REQUEST,
		});
	};

	let song_reservations = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1 AND r.user_id = $2",
		ReservationType::Song as i32,
		owner.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.map(|reservation| Reservation {
		user: User {
			id: reservation.user_id,
			name: reservation.name.clone(),
			avatar: reservation.avatar.clone(),
			display_name: reservation.display_name.clone(),
			public_likes: reservation.public_likes,
			theme: reservation.theme.into(),
		},
		reservation_type: reservation.reservation_type.into(),
		range_start: reservation.range_start,
		length: reservation.length,
		time: reservation.time.assume_offset(time::UtcOffset::UTC),
	})
	.flat_map(|reservation| {
		(reservation.range_start..(reservation.range_start + reservation.length))
			.map(move |i| (i, reservation.clone()))
	})
	.collect::<BTreeMap<_, _>>();

	let module_reservations = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1 AND r.user_id = $2",
		ReservationType::Module as i32,
		owner.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.map(|reservation| Reservation {
		user: User {
			id: reservation.user_id,
			name: reservation.name.clone(),
			avatar: reservation.avatar.clone(),
			display_name: reservation.display_name.clone(),
			public_likes: reservation.public_likes,
			theme: reservation.theme.into(),
		},
		reservation_type: reservation.reservation_type.into(),
		range_start: reservation.range_start,
		length: reservation.length,
		time: reservation.time.assume_offset(time::UtcOffset::UTC),
	})
	.flat_map(|reservation| {
		(reservation.range_start..(reservation.range_start + reservation.length))
			.map(move |i| (i, reservation.clone()))
	})
	.collect::<BTreeMap<_, _>>();

	let cstm_item_reservations = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1 AND r.user_id = $2",
		ReservationType::CstmItem as i32,
		owner.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.map(|reservation| Reservation {
		user: User {
			id: reservation.user_id,
			name: reservation.name.clone(),
			avatar: reservation.avatar.clone(),
			display_name: reservation.display_name.clone(),
			public_likes: reservation.public_likes,
			theme: reservation.theme.into(),
		},
		reservation_type: reservation.reservation_type.into(),
		range_start: reservation.range_start,
		length: reservation.length,
		time: reservation.time.assume_offset(time::UtcOffset::UTC),
	})
	.flat_map(|reservation| {
		(reservation.range_start..(reservation.range_start + reservation.length))
			.map(move |i| (i, reservation.clone()))
	})
	.collect::<BTreeMap<_, _>>();

	Ok(UserReservationsTemplate {
		base,
		owner,
		song_reservations,
		module_reservations,
		cstm_item_reservations,
	})
}

#[derive(Template, WebTemplate)]
#[template(path = "upload.html")]
struct UploadTemplate {
	base: BaseTemplate,
	update: Option<Post>,
	jwt: String,
	user: User,
}

async fn upload(
	base: BaseTemplate,
	update_id: Option<Path<i32>>,
	user: User,
	State(state): State<AppState>,
) -> Result<UploadTemplate, ErrorTemplate> {
	let Some(jwt) = base.jwt.clone() else {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::UNAUTHORIZED,
		});
	};

	let post = if let Some(Path(id)) = update_id {
		if let Some(post) = Post::get_full(id, &state.db).await {
			if post.authors.contains(&user) {
				Some(post)
			} else {
				return Err(ErrorTemplate {
					base,
					status: StatusCode::UNAUTHORIZED,
				});
			}
		} else {
			return Err(ErrorTemplate {
				base,
				status: StatusCode::UNAUTHORIZED,
			});
		}
	} else {
		None
	};

	Ok(UploadTemplate {
		base,
		update: post,
		jwt,
		user,
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
	pvs: PvSearch,
	modules: ModuleSearch,
	cstm_items: CstmItemSearch,
	pv_easy_count: usize,
	pv_normal_count: usize,
	pv_hard_count: usize,
	pv_extreme_count: usize,
	pv_exextreme_count: usize,
	conflicting_pvs: PvSearch,
	conflicting_modules: ModuleSearch,
	conflicting_cstm_items: CstmItemSearch,
	conflicting_pv_reservations: BTreeMap<User, Vec<i32>>,
	conflicting_module_reservations: BTreeMap<User, Vec<i32>>,
	conflicting_cstm_item_reservations: BTreeMap<User, Vec<i32>>,
	body_markdown: String,
}

async fn post_redirect(Path(id): Path<i32>) -> Redirect {
	Redirect::permanent(&format!("https://divamodarchive.com/post/{id}"))
}

async fn post_detail(
	Path(id): Path<i32>,
	State(state): State<AppState>,
	base: BaseTemplate,
) -> Result<PostTemplate, ErrorTemplate> {
	let Some(post) = Post::get_full(id, &state.db).await else {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::NOT_FOUND,
		});
	};

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
		post.authors.iter().any(|u| u.id == user.id)
	} else {
		false
	};

	let Json(pvs) = search_pvs(
		Query(SearchParams {
			query: None,
			filter: Some(format!("post={}", post.id)),
			limit: Some(2000),
			offset: Some(0),
		}),
		State(state.clone()),
	)
	.await
	.unwrap_or_default();

	let Json(modules) = search_modules(
		Query(SearchParams {
			query: None,
			filter: Some(format!("post_id={}", post.id)),
			limit: Some(2000),
			offset: Some(0),
		}),
		State(state.clone()),
	)
	.await
	.unwrap_or_default();

	let Json(cstm_items) = search_cstm_items(
		Query(SearchParams {
			query: None,
			filter: Some(format!("post_id={}", post.id)),
			limit: Some(2000),
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

	let conflicting_pvs = if pvs.pvs.len() > 0 {
		let filter = pvs
			.pvs
			.iter()
			.map(|pv| format!("(pv_id={} AND post!={})", pv.id, pv.post.unwrap_or(-1)))
			.intersperse(String::from(" OR "))
			.collect::<String>();

		let Json(conflicting_pvs) = search_pvs(
			Query(SearchParams {
				query: None,
				filter: Some(filter),
				limit: Some(2000),
				offset: Some(0),
			}),
			State(state.clone()),
		)
		.await
		.unwrap_or_default();

		conflicting_pvs
	} else {
		PvSearch::default()
	};

	let conflicting_modules = if modules.modules.len() > 0 {
		let filter = modules
			.modules
			.iter()
			.map(|module| {
				format!(
					"(module_id={} AND post_id!={})",
					module.id,
					module.post.unwrap_or(-1)
				)
			})
			.intersperse(String::from(" OR "))
			.collect::<String>();

		let Json(conflicting_modules) = search_modules(
			Query(SearchParams {
				query: None,
				filter: Some(filter),
				limit: Some(2000),
				offset: Some(0),
			}),
			State(state.clone()),
		)
		.await
		.unwrap_or_default();

		conflicting_modules
	} else {
		ModuleSearch::default()
	};

	let conflicting_cstm_items = if cstm_items.cstm_items.len() > 0 {
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

		let Json(conflicting_cstm_items) = search_cstm_items(
			Query(SearchParams {
				query: None,
				filter: Some(filter),
				limit: Some(2000),
				offset: Some(0),
			}),
			State(state.clone()),
		)
		.await
		.unwrap_or_default();

		conflicting_cstm_items
	} else {
		CstmItemSearch::default()
	};

	let mut conflicting_pv_reservations: BTreeMap<User, Vec<i32>> = BTreeMap::new();
	let mut conflicting_module_reservations: BTreeMap<User, Vec<i32>> = BTreeMap::new();
	let mut conflicting_cstm_item_reservations: BTreeMap<User, Vec<i32>> = BTreeMap::new();

	for pv in &pvs.pvs {
		let users = sqlx::query_as!(
			User,
			r#"
			SELECT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme
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

			if let Some(conflict) = conflicting_pv_reservations.get_mut(&user) {
				conflict.push(pv.id);
			} else {
				conflicting_pv_reservations.insert(user, vec![pv.id]);
			}
		}
	}

	for module in &modules.modules {
		let reservations = sqlx::query!(
			r#"
			SELECT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme
			FROM reservations r
			LEFT JOIN users u ON r.user_id = u.id
			WHERE r.reservation_type = 1
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
		for reservation in reservations {
			let user = User {
				id: reservation.id,
				name: reservation.name,
				avatar: reservation.avatar,
				display_name: reservation.display_name,
				public_likes: reservation.public_likes,
				theme: reservation.theme.into(),
			};
			if post.authors.contains(&user) {
				continue;
			}

			if let Some(conflict) = conflicting_module_reservations.get_mut(&user) {
				conflict.push(module.id);
			} else {
				conflicting_module_reservations.insert(user, vec![module.id]);
			}
		}
	}

	for cstm_item in &cstm_items.cstm_items {
		let reservations = sqlx::query!(
			r#"
			SELECT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme
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
		for reservation in reservations {
			let user = User {
				id: reservation.id,
				name: reservation.name,
				avatar: reservation.avatar,
				display_name: reservation.display_name,
				public_likes: reservation.public_likes,
				theme: reservation.theme.into(),
			};
			if post.authors.contains(&user) {
				continue;
			}

			if let Some(conflict) = conflicting_cstm_item_reservations.get_mut(&user) {
				conflict.push(cstm_item.id);
			} else {
				conflicting_cstm_item_reservations.insert(user, vec![cstm_item.id]);
			}
		}
	}

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

	let body_markdown = comrak::markdown_to_html(&post.text, &options)
		.replace("<img src", "<img style=\"width: 100%\" src");

	Ok(PostTemplate {
		user: base.user.clone(),
		jwt: base.jwt.clone(),
		has_liked,
		is_author,
		base,
		post,
		config: state.config,
		pvs,
		modules,
		cstm_items,
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
		conflicting_cstm_item_reservations,
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
		ORDER BY time DESC
		LIMIT 20
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

	Ok(ReportTemplate { base, post })
}

#[derive(Template, WebTemplate)]
#[template(path = "pvs.html")]
struct PvsTemplate {
	base: BaseTemplate,
	pvs: PvSearch,
}

async fn pvs(base: BaseTemplate, State(state): State<AppState>) -> PvsTemplate {
	let Json(pvs) = search_pvs(
		Query(SearchParams {
			query: None,
			filter: None,
			limit: Some(20),
			offset: Some(0),
		}),
		State(state),
	)
	.await
	.unwrap_or_default();

	return PvsTemplate { base, pvs };
}

#[derive(Template, WebTemplate)]
#[template(path = "modules.html")]
struct ModulesTemplate {
	base: BaseTemplate,
	modules: ModuleSearch,
}

async fn modules(base: BaseTemplate, State(state): State<AppState>) -> ModulesTemplate {
	let Json(modules) = search_modules(
		Query(SearchParams {
			query: None,
			filter: None,
			limit: Some(20),
			offset: Some(0),
		}),
		State(state),
	)
	.await
	.unwrap_or_default();

	return ModulesTemplate { base, modules };
}

#[derive(Template, WebTemplate)]
#[template(path = "cstm_items.html")]
struct CstmItemsTemplate {
	base: BaseTemplate,
	cstm_items: CstmItemSearch,
}

async fn cstm_items(base: BaseTemplate, State(state): State<AppState>) -> CstmItemsTemplate {
	let Json(cstm_items) = search_cstm_items(
		Query(SearchParams {
			query: None,
			filter: None,
			limit: Some(20),
			offset: Some(0),
		}),
		State(state),
	)
	.await
	.unwrap_or_default();

	return CstmItemsTemplate { base, cstm_items };
}

// This code fucking sucks
#[derive(Template, WebTemplate)]
#[template(path = "pv_spreadsheet.html")]
struct PvSpreadsheet {
	base: BaseTemplate,
	reservations: BTreeMap<i32, Reservation>,
	pvs: BTreeMap<i32, Vec<Pv>>,
	posts: BTreeMap<i32, Post>,
}

async fn pv_spreadsheet(base: BaseTemplate, State(state): State<AppState>) -> PvSpreadsheet {
	let mut reservations = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1",
		ReservationType::Song as i32,
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.map(|reservation| Reservation {
		user: User {
			id: reservation.user_id,
			name: reservation.name.clone(),
			avatar: reservation.avatar.clone(),
			display_name: reservation.display_name.clone(),
			public_likes: reservation.public_likes,
			theme: reservation.theme.into(),
		},
		reservation_type: reservation.reservation_type.into(),
		range_start: reservation.range_start,
		length: reservation.length,
		time: reservation.time.assume_offset(time::UtcOffset::UTC),
	})
	.flat_map(|reservation| {
		(reservation.range_start..(reservation.range_start + reservation.length))
			.map(move |i| (i, reservation.clone()))
	})
	.collect::<BTreeMap<_, _>>();

	let Json(search) = search_pvs(
		Query(SearchParams {
			query: None,
			filter: None,
			limit: Some(100_000),
			offset: Some(0),
		}),
		State(state.clone()),
	)
	.await
	.unwrap_or_default();

	let mut pvs: BTreeMap<i32, Vec<Pv>> = BTreeMap::new();
	for pv in &search.pvs {
		if let Some(reservation) = reservations.get(&pv.id) {
			if let Some(post) = pv.post {
				if search.posts[&post].authors.contains(&reservation.user) {
					reservations.remove(&pv.id);
				}
			}
		}
		if let Some(original) = pvs.get_mut(&pv.id) {
			original.push(pv.clone());
		} else {
			pvs.insert(pv.id, vec![pv.clone()]);
		}
	}

	PvSpreadsheet {
		base,
		reservations,
		pvs,
		posts: search.posts,
	}
}

#[derive(Template, WebTemplate)]
#[template(path = "reserve.html")]
struct ReserveTemplate {
	base: BaseTemplate,
	remaining_song_reservations: i32,
	remaining_module_reservations: i32,
	remaining_cstm_item_reservations: i32,
}

async fn reserve(base: BaseTemplate, user: User, State(state): State<AppState>) -> ReserveTemplate {
	let remaining_song_reservations =
		get_user_max_reservations(ReservationType::Song, &user, &state).await;
	let remaining_module_reservations =
		get_user_max_reservations(ReservationType::Module, &user, &state).await;
	let remaining_cstm_item_reservations =
		get_user_max_reservations(ReservationType::CstmItem, &user, &state).await;
	ReserveTemplate {
		base,
		remaining_song_reservations,
		remaining_module_reservations,
		remaining_cstm_item_reservations,
	}
}
