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
use base64::prelude::*;
use itertools::*;
use std::collections::*;

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
		.route("/reservations/{id}", get(user_reservations))
		.route("/upload", get(upload))
		.route("/settings", get(settings))
		.route("/pvs", get(pvs))
		.route("/modules", get(modules))
		.route("/cstm_items", get(cstm_items))
		.route("/pv_spreadsheet", get(pv_spreadsheet))
		.route("/sprite_spreadsheet", get(sprite_spreadsheet))
		.route("/aet_spreadsheet", get(aet_spreadsheet))
		.route("/objset_spreadsheet", get(objset_spreadsheet))
		.route("/texture_spreadsheet", get(texture_spreadsheet))
		.route("/reserve", get(reserve))
		//.route("/admin", get(admin))
		.layer(axum::middleware::from_fn(axum_html_minifier::html_minifier))
		.with_state(state)
}

const DIFFICULTY_COLOURS: [&'static str; 5] = [
	"--diva-easy",
	"--diva-normal",
	"--diva-hard",
	"--diva-extreme",
	"--diva-exex",
];

mod filters {
	use askama::filters::*;

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

	pub fn autolink<T: std::fmt::Display>(
		s: T,
		_: &dyn askama::Values,
	) -> askama::Result<Safe<String>> {
		let escaped = format!("{}", escape(s, Html)?);
		let re = regex::Regex::new(
			"https?:\\/\\/[-a-zA-Z0-9@:%._\\+~#=]{1,256}\\.[a-zA-Z]{1,6}\\b(?:[-a-zA-Z0-9()@:%_\\+.~#?&\\/=]*)",
		).unwrap();

		let mut output = escaped.clone();
		for m in re.find_iter(&escaped) {
			let m = m.as_str();
			output = output.replace(m, &format!("<a href=\"{m}\">{m}</a>"));
		}

		Ok(Safe(output))
	}
}

#[derive(Clone)]
pub struct BaseTemplate {
	pub user: Option<User>,
	pub config: Config,
	pub jwt: Option<String>,
	pub report_count: Option<i64>,
	pub has_reservations: bool,
	pub has_likes: bool,
	pub pending_upload: Option<Post>,
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

		Ok(Self {
			user,
			config: state.config,
			jwt,
			report_count,
			has_reservations,
			has_likes,
			pending_upload,
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
		has_likes,
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

	let mut users = BTreeMap::new();

	let mut song_reservations = sqlx::query!(
		"SELECT * FROM reservations r WHERE reservation_type = $1 AND r.user_id = $2",
		ReservationType::Song as i32,
		owner.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.flat_map(|reservation| {
		(reservation.range_start..(reservation.range_start + reservation.length)).map(move |i| {
			(
				i,
				Reservation {
					user: reservation.user_id,
					reservation_type: reservation.reservation_type.into(),
					time: reservation.time.assume_offset(time::UtcOffset::UTC),
					label: None,
				},
			)
		})
	})
	.collect::<BTreeMap<_, _>>();

	let mut module_reservations = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1 AND r.user_id = $2",
		ReservationType::Module as i32,
		owner.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.flat_map(|reservation| {
		if !users.contains_key(&reservation.user_id) {
			users.insert(
				reservation.user_id,
				User {
					id: reservation.user_id,
					name: reservation.name.clone(),
					avatar: reservation.avatar.clone(),
					display_name: reservation.display_name.clone(),
					public_likes: reservation.public_likes,
					theme: reservation.theme.into(),
				},
			);
		}
		(reservation.range_start..(reservation.range_start + reservation.length)).map(move |i| {
			(
				i,
				Reservation {
					user: reservation.user_id,
					reservation_type: reservation.reservation_type.into(),
					time: reservation.time.assume_offset(time::UtcOffset::UTC),
					label: None,
				},
			)
		})
	})
	.collect::<BTreeMap<_, _>>();

	let mut cstm_item_reservations = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1 AND r.user_id = $2",
		ReservationType::CstmItem as i32,
		owner.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.flat_map(|reservation| {
		if !users.contains_key(&reservation.user_id) {
			users.insert(
				reservation.user_id,
				User {
					id: reservation.user_id,
					name: reservation.name.clone(),
					avatar: reservation.avatar.clone(),
					display_name: reservation.display_name.clone(),
					public_likes: reservation.public_likes,
					theme: reservation.theme.into(),
				},
			);
		}
		(reservation.range_start..(reservation.range_start + reservation.length)).map(move |i| {
			(
				i,
				Reservation {
					user: reservation.user_id,
					reservation_type: reservation.reservation_type.into(),
					time: reservation.time.assume_offset(time::UtcOffset::UTC),
					label: None,
				},
			)
		})
	})
	.collect::<BTreeMap<_, _>>();

	for record in sqlx::query!(
		"SELECT rl.id, rl.user_id, rl.label, rl.reservation_type FROM reservation_labels rl"
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	{
		let reservation_type: ReservationType = record.reservation_type.into();
		match reservation_type {
			ReservationType::Song => {
				let Some(reservation) = song_reservations.get_mut(&record.id) else {
					continue;
				};
				if reservation.user != record.user_id {
					continue;
				};
				reservation.label = Some(record.label.clone());
			}
			ReservationType::Module => {
				let Some(reservation) = module_reservations.get_mut(&record.id) else {
					continue;
				};
				if reservation.user != record.user_id {
					continue;
				};
				reservation.label = Some(record.label.clone());
			}
			ReservationType::CstmItem => {
				let Some(reservation) = cstm_item_reservations.get_mut(&record.id) else {
					continue;
				};
				if reservation.user != record.user_id {
					continue;
				};
				reservation.label = Some(record.label.clone());
			}
		}
	}

	Ok(UserReservationsTemplate {
		base,
		owner,
		song_reservations,
		module_reservations,
		cstm_item_reservations,
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
	nc_songs: NcSongSearch,
	pv_easy_count: usize,
	pv_normal_count: usize,
	pv_hard_count: usize,
	pv_extreme_count: usize,
	pv_exextreme_count: usize,
	conflicting_pvs: PvSearch,
	conflicting_modules: ModuleSearch,
	conflicting_cstm_items: CstmItemSearch,
	conflicting_pv_reservations: BTreeMap<User, Vec<(i32, String)>>,
	conflicting_module_reservations: BTreeMap<User, Vec<(i32, String)>>,
	conflicting_cstm_item_reservations: BTreeMap<User, Vec<(i32, String)>>,
	conflicting_sprites: BTreeMap<i32, Vec<(u32, String, String)>>,
	conflicting_aets: BTreeMap<i32, Vec<(u32, String, String)>>,
	conflicting_objsets: BTreeMap<i32, Vec<(u32, String, String)>>,
	conflicting_textures: BTreeMap<i32, Vec<(u32, String, String)>>,
	conflicting_db_posts: BTreeMap<i32, Post>,
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

	if post.private {
		if !base.user.as_ref().map_or(false, |user| {
			post.authors.contains(user) || user.is_admin(&state.config)
		}) {
			return Err(ErrorTemplate {
				base,
				status: StatusCode::UNAUTHORIZED,
			});
		}
	}

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

	let Json(pvs) = search_pvs(
		Query(SearchParams {
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
		Query(SearchParams {
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
		Query(SearchParams {
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
		Query(SearchParams {
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
				limit: Some(u32::MAX as usize),
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
				limit: Some(u32::MAX as usize),
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
				limit: Some(u32::MAX as usize),
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

	let mut conflicting_pv_reservations: BTreeMap<User, Vec<(i32, String)>> = BTreeMap::new();
	let mut conflicting_module_reservations: BTreeMap<User, Vec<(i32, String)>> = BTreeMap::new();
	let mut conflicting_cstm_item_reservations: BTreeMap<User, Vec<(i32, String)>> =
		BTreeMap::new();

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

			if let Some(conflict) = conflicting_pv_reservations.get_mut(&user) {
				conflict.push((pv.id, label));
			} else {
				conflicting_pv_reservations.insert(user, vec![(pv.id, label)]);
			}
		}
	}

	for module in &modules.modules {
		let users = sqlx::query_as!(
			User,
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

			if let Some(conflict) = conflicting_module_reservations.get_mut(&user) {
				conflict.push((module.id, label));
			} else {
				conflicting_module_reservations.insert(user, vec![(module.id, label)]);
			}
		}
	}

	for cstm_item in &cstm_items.cstm_items {
		let users = sqlx::query_as!(
			User,
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

			if let Some(conflict) = conflicting_cstm_item_reservations.get_mut(&user) {
				conflict.push((cstm_item.id, label));
			} else {
				conflicting_cstm_item_reservations.insert(user, vec![(cstm_item.id, label)]);
			}
		}
	}

	let mut conflicting_sprites: BTreeMap<i32, Vec<(u32, String, String)>> = BTreeMap::new();
	let mut conflicting_aets: BTreeMap<i32, Vec<(u32, String, String)>> = BTreeMap::new();
	let mut conflicting_objsets: BTreeMap<i32, Vec<(u32, String, String)>> = BTreeMap::new();
	let mut conflicting_textures: BTreeMap<i32, Vec<(u32, String, String)>> = BTreeMap::new();
	let mut conflicting_db_posts = BTreeMap::new();

	if let Ok(entries) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("sprites"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry))
					.collect::<BTreeMap<_, _>>()
			}) {
		let search = entries
			.iter()
			.map(|(id, entry)| format!("(id={} AND name!={})", id, entry.name))
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
				if let Some(existing) = conflicting_sprites.get_mut(&conflict.post_id) {
					existing.push((
						conflict.id,
						entries[&conflict.id].name.clone(),
						conflict.name,
					));
				} else {
					conflicting_sprites.insert(
						conflict.post_id,
						vec![(
							conflict.id,
							entries[&conflict.id].name.clone(),
							conflict.name,
						)],
					);
				}

				if conflict.post_id != -1 && !conflicting_db_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflicting_db_posts.insert(post.id, post);
					}
				}
			}
		}
	}

	if let Ok(entries) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aets"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry))
					.collect::<BTreeMap<_, _>>()
			}) {
		let search = entries
			.iter()
			.map(|(id, entry)| format!("(id={} AND name!={})", id, entry.name))
			.intersperse(String::from(" OR "))
			.collect::<String>();

		if let Ok(conflicts) =
			meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aets"))
				.with_limit(u32::MAX as usize)
				.with_filter(&format!("({search}) AND post_id!={}", post.id))
				.execute::<MeilisearchDbEntry>()
				.await
		{
			for conflict in conflicts.results {
				if let Some(existing) = conflicting_aets.get_mut(&conflict.post_id) {
					existing.push((
						conflict.id,
						entries[&conflict.id].name.clone(),
						conflict.name,
					));
				} else {
					conflicting_aets.insert(
						conflict.post_id,
						vec![(
							conflict.id,
							entries[&conflict.id].name.clone(),
							conflict.name,
						)],
					);
				}

				if conflict.post_id != -1 && !conflicting_db_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflicting_db_posts.insert(post.id, post);
					}
				}
			}
		}
	}

	if let Ok(entries) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("objsets"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry))
					.collect::<BTreeMap<_, _>>()
			}) {
		let search = entries
			.iter()
			.map(|(id, entry)| format!("(id={} AND name!={})", id, entry.name))
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
				if let Some(existing) = conflicting_objsets.get_mut(&conflict.post_id) {
					existing.push((
						conflict.id,
						entries[&conflict.id].name.clone(),
						conflict.name,
					));
				} else {
					conflicting_objsets.insert(
						conflict.post_id,
						vec![(
							conflict.id,
							entries[&conflict.id].name.clone(),
							conflict.name,
						)],
					);
				}

				if conflict.post_id != -1 && !conflicting_db_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflicting_db_posts.insert(post.id, post);
					}
				}
			}
		}
	}

	if let Ok(entries) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("textures"))
			.with_limit(u32::MAX as usize)
			.with_filter(&format!("post_id={}", post.id))
			.execute::<MeilisearchDbEntry>()
			.await
			.map(|entries| {
				entries
					.results
					.into_iter()
					.map(|entry| (entry.id, entry))
					.collect::<BTreeMap<_, _>>()
			}) {
		let search = entries
			.iter()
			.map(|(id, entry)| format!("(id={} AND name!={})", id, entry.name))
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
				if let Some(existing) = conflicting_textures.get_mut(&conflict.post_id) {
					existing.push((
						conflict.id,
						entries[&conflict.id].name.clone(),
						conflict.name,
					));
				} else {
					conflicting_textures.insert(
						conflict.post_id,
						vec![(
							conflict.id,
							entries[&conflict.id].name.clone(),
							conflict.name,
						)],
					);
				}

				if conflict.post_id != -1 && !conflicting_db_posts.contains_key(&conflict.post_id) {
					if let Some(post) = Post::get_short(conflict.post_id, &state.db).await {
						conflicting_db_posts.insert(post.id, post);
					}
				}
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
		nc_songs,
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
		conflicting_sprites,
		conflicting_aets,
		conflicting_objsets,
		conflicting_textures,
		conflicting_db_posts,
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
	if post.private {
		return Err(ErrorTemplate {
			base,
			status: StatusCode::UNAUTHORIZED,
		});
	}

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

#[derive(Template, WebTemplate)]
#[template(path = "pv_spreadsheet.html")]
struct PvSpreadsheet {
	base: BaseTemplate,
	reservations: HashMap<i32, Reservation>,
	users: HashMap<i64, User>,
	pvs: HashMap<i32, Vec<Pv>>,
	posts: BTreeMap<i32, Post>,
	last: i32,
}

async fn pv_spreadsheet(base: BaseTemplate, State(state): State<AppState>) -> PvSpreadsheet {
	let mut users = HashMap::new();

	let mut reservations = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1",
		ReservationType::Song as i32,
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.flat_map(|reservation| {
		if !users.contains_key(&reservation.user_id) {
			users.insert(
				reservation.user_id,
				User {
					id: reservation.user_id,
					name: reservation.name.clone(),
					avatar: reservation.avatar.clone(),
					display_name: reservation.display_name.clone(),
					public_likes: reservation.public_likes,
					theme: reservation.theme.into(),
				},
			);
		}
		(reservation.range_start..(reservation.range_start + reservation.length)).map(move |i| {
			(
				i,
				Reservation {
					user: reservation.user_id,
					reservation_type: reservation.reservation_type.into(),
					time: reservation.time.assume_offset(time::UtcOffset::UTC),
					label: None,
				},
			)
		})
	})
	.collect::<HashMap<_, _>>();

	let mut pvs: HashMap<i32, Vec<Pv>> = HashMap::new();
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();

	if let Ok(search) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("pvs"))
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchPv>()
			.await
	{
		for pv in search.results {
			let post = if pv.post == -1 {
				None
			} else if let Some(post) = posts.get(&pv.post) {
				Some(post.id)
			} else if let Some(post) = Post::get_full(pv.post, &state.db).await {
				posts.insert(post.id, post.clone());
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

			if let Some(original) = pvs.get_mut(&pv.id) {
				original.push(pv);
			} else {
				pvs.insert(pv.id, vec![pv]);
			}
		}
	};

	for record in sqlx::query!(
		"SELECT rl.id, rl.user_id, rl.label, rl.reservation_type FROM reservation_labels rl WHERE reservation_type = $1",
		ReservationType::Song as i32,
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	{
		if let Some(reservation) = reservations.get_mut(&record.id) {
			if reservation.user != record.user_id
			{
				continue;
			};
			reservation.label = Some(record.label.clone());
		}
	}

	let reservation_set = reservations.keys().collect::<BTreeSet<_>>();
	let largest_reservation = reservation_set.last().unwrap_or(&&1000);
	let pv_set = pvs.keys().collect::<BTreeSet<_>>();
	let largest_pv = pv_set.last().unwrap_or(&&1000);

	let last = if largest_reservation >= largest_pv {
		**largest_reservation
	} else {
		**largest_pv
	};

	PvSpreadsheet {
		base,
		reservations,
		users,
		pvs,
		posts,
		last,
	}
}

#[derive(Template, WebTemplate)]
#[template(path = "reserve.html")]
struct ReserveTemplate {
	base: BaseTemplate,
	remaining_song_reservations: usize,
	remaining_module_reservations: usize,
	remaining_cstm_item_reservations: usize,
	existing_song_reservations: usize,
	existing_module_reservations: usize,
	existing_cstm_item_reservations: usize,
	uploaded_songs: usize,
	uploaded_modules: usize,
	uploaded_cstm_items: usize,
}

async fn reserve(base: BaseTemplate, user: User, State(state): State<AppState>) -> ReserveTemplate {
	ReserveTemplate {
		base,
		remaining_song_reservations: get_user_max_reservations(
			ReservationType::Song,
			&user,
			&state,
		)
		.await,
		remaining_module_reservations: get_user_max_reservations(
			ReservationType::Module,
			&user,
			&state,
		)
		.await,
		remaining_cstm_item_reservations: get_user_max_reservations(
			ReservationType::CstmItem,
			&user,
			&state,
		)
		.await,
		existing_song_reservations: get_user_reservations(ReservationType::Song, &user, &state)
			.await
			.len(),
		existing_module_reservations: get_user_reservations(ReservationType::Module, &user, &state)
			.await
			.len(),
		existing_cstm_item_reservations: get_user_reservations(
			ReservationType::CstmItem,
			&user,
			&state,
		)
		.await
		.len(),
		uploaded_songs: get_user_uploads(ReservationType::Song, &user, &state)
			.await
			.len(),
		uploaded_modules: get_user_uploads(ReservationType::Module, &user, &state)
			.await
			.len(),
		uploaded_cstm_items: get_user_uploads(ReservationType::CstmItem, &user, &state)
			.await
			.len(),
	}
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
		has_reservations: base.has_reservations,
		has_likes: base.has_likes,
		pending_upload: None,
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

#[derive(Template, WebTemplate)]
#[template(path = "db_spreadsheet.html")]
struct DbSpreadsheetTemplate {
	base: BaseTemplate,
	human_name: String,
	entries: Vec<MeilisearchDbEntry>,
	posts: BTreeMap<i32, Post>,
}

async fn sprite_spreadsheet(
	base: BaseTemplate,
	State(state): State<AppState>,
) -> Result<DbSpreadsheetTemplate, ErrorTemplate> {
	db_spreadsheet(String::from("Sprite"), String::from("sprites"), base, state).await
}

async fn aet_spreadsheet(
	base: BaseTemplate,
	State(state): State<AppState>,
) -> Result<DbSpreadsheetTemplate, ErrorTemplate> {
	db_spreadsheet(String::from("Aet"), String::from("aets"), base, state).await
}

async fn objset_spreadsheet(
	base: BaseTemplate,
	State(state): State<AppState>,
) -> Result<DbSpreadsheetTemplate, ErrorTemplate> {
	db_spreadsheet(String::from("Objset"), String::from("objsets"), base, state).await
}

async fn texture_spreadsheet(
	base: BaseTemplate,
	State(state): State<AppState>,
) -> Result<DbSpreadsheetTemplate, ErrorTemplate> {
	db_spreadsheet(
		String::from("Texture"),
		String::from("textures"),
		base,
		state,
	)
	.await
}

async fn db_spreadsheet(
	human_name: String,
	index: String,
	base: BaseTemplate,
	state: AppState,
) -> Result<DbSpreadsheetTemplate, ErrorTemplate> {
	let Ok(entries) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index(index))
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchDbEntry>()
			.await
	else {
		return Err(ErrorTemplate {
			base: base.clone(),
			status: StatusCode::INTERNAL_SERVER_ERROR,
		});
	};

	let filter = entries
		.results
		.iter()
		.filter(|entry| entry.post_id != -1)
		.map(|entry| entry.post_id)
		.collect::<BTreeSet<_>>()
		.into_iter()
		.map(|id| format!("id={}", id))
		.intersperse(String::from(" OR "))
		.collect::<String>();

	let Json(posts) = crate::api::posts::search_posts(
		Query(crate::api::posts::SearchParams {
			query: None,
			sort: None,
			filter: Some(filter),
			limit: Some(u32::MAX as usize),
			offset: Some(0),
		}),
		State(state.clone()),
	)
	.await
	.unwrap_or_default();

	let posts = posts
		.into_iter()
		.map(|post| (post.id, post))
		.collect::<BTreeMap<_, _>>();

	let mut entries = entries.results;
	entries.sort_by(|a, b| a.id.cmp(&b.id));

	Ok(DbSpreadsheetTemplate {
		base,
		human_name,
		entries,
		posts,
	})
}
