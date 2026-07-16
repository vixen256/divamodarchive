use crate::AppState;
use axum::{Router, routing::*};
use posts::*;
use utoipa::OpenApi;

pub mod posts;

#[derive(OpenApi)]
#[openapi(paths(search_posts, count_posts, get_post))]
struct ApiDoc;

pub fn route(state: AppState) -> Router {
	Router::new()
		.merge(utoipa_swagger_ui::SwaggerUi::new("/api/v1").url("/api/v1.json", ApiDoc::openapi()))
		.route("/api/v1/posts", get(search_posts).post(create_post))
		.route("/api/v1/posts/count", get(count_posts))
		.route(
			"/api/v1/posts/{id}",
			get(get_post).delete(delete_post).patch(edit_post),
		)
		.route("/api/v1/posts/posts", get(get_multiple_posts))
		.route("/api/v1/posts/upload_image", get(upload_image))
		.route("/api/v1/posts/start_upload", post(create_pending_upload))
		.route(
			"/api/v1/posts/continue_upload",
			get(continue_pending_upload),
		)
		.route("/api/v1/posts/{id}/image/{index}", delete(remove_image))
		.route(
			"/api/v1/posts/{id}/image",
			post(append_image).patch(swap_images),
		)
		.route(
			"/api/v1/posts/{id}/download/{variant}",
			get(download).head(download_head),
		)
		.route("/api/v1/posts/{id}/like", post(like))
		.route("/api/v1/posts/{id}/comment", post(comment))
		.route(
			"/api/v1/posts/{id}/author",
			post(add_author).delete(remove_author),
		)
		.route(
			"/api/v1/posts/{id}/dependency",
			post(add_dependency)
				.put(set_dependency_description)
				.delete(remove_dependency),
		)
		.route("/api/v1/posts/{id}/report", post(report))
		.route(
			"/api/v1/posts/{post}/comment/{comment}",
			delete(delete_comment),
		)
		.route("/api/v1/users/settings", post(user_settings))
		.layer(tower_http::cors::CorsLayer::permissive())
		.with_state(state)
}
