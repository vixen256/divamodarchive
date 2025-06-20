use crate::AppState;
use axum::{Router, routing::*};
use ids::*;
use posts::*;

pub mod ids;
pub mod posts;

pub fn route(state: AppState) -> Router {
	Router::new()
		.route("/api/v1/posts", get(search_posts))
		.route("/api/v1/posts/count", get(count_posts))
		.route("/api/v1/posts/{id}", get(get_post).delete(delete_post))
		.route("/api/v1/posts/posts", get(get_multiple_posts))
		.route("/api/v1/posts/edit", post(edit))
		.route("/api/v1/posts/upload_image", get(upload_image))
		.route("/api/v1/posts/upload", get(upload_ws))
		.route("/api/v1/posts/{id}/download/{variant}", get(download))
		.route("/api/v1/posts/{id}/like", post(like))
		.route("/api/v1/posts/{id}/comment", post(comment))
		.route("/api/v1/posts/{id}/author", post(add_author))
		.route("/api/v1/posts/{id}/dependency", post(add_dependency))
		.route("/api/v1/posts/{id}/report", post(report))
		.route("/api/v1/posts/{id}/extract", post(extract_post))
		.route(
			"/api/v1/posts/{post}/comment/{comment}",
			delete(delete_comment),
		)
		.route("/api/v1/users/settings", post(user_settings))
		.route("/api/v1/ids/pvs", get(search_pvs))
		.route("/api/v1/ids/modules", get(search_modules))
		.route("/api/v1/ids/cstm_items", get(search_cstm_items))
		.route("/api/v1/reserve/check", get(web_check_reserve_range))
		.route("/api/v1/reserve/find", get(web_find_reserve_range))
		.route(
			"/api/v1/reserve",
			post(create_reservation).delete(delete_reservation),
		)
		.route("/api/v1/reservations/{id}/label", post(label_reservation))
		.route("/api/v1/reservations", get(reservations_json))
		.layer(tower_http::cors::CorsLayer::permissive())
		.with_state(state)
}
