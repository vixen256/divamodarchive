use crate::AppState;
use axum::{Router, routing::*};
use ids::*;
use posts::*;
use utoipa::OpenApi;

pub mod ids;
pub mod posts;

#[derive(OpenApi)]
#[openapi(paths(
	search_posts,
	count_posts,
	get_post,
	post_detail,
	search_pvs,
	search_modules,
	search_cstm_items,
	search_nc_songs,
	all_pvs,
	all_modules,
	all_cstm_items,
	all_sprites,
	all_aet_sets,
	all_aet_scenes,
	all_objsets,
	all_textures
))]
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
		.route("/api/v1/posts/{id}/detail", get(post_detail))
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
			post(add_dependency).delete(remove_dependency),
		)
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
		.route("/api/v1/ids/nc_songs", get(search_nc_songs))
		.route("/api/v1/ids/all_pvs", get(all_pvs))
		.route("/api/v1/ids/all_modules", get(all_modules))
		.route("/api/v1/ids/all_cstm_items", get(all_cstm_items))
		.route("/api/v1/ids/all_sprites", get(all_sprites))
		.route("/api/v1/ids/all_aet_sets", get(all_aet_sets))
		.route("/api/v1/ids/all_aet_scenes", get(all_aet_scenes))
		.route("/api/v1/ids/all_objsets", get(all_objsets))
		.route("/api/v1/ids/all_textures", get(all_textures))
		.route("/api/v1/reserve/check", get(web_check_reserve_range))
		.route("/api/v1/reserve/find", get(web_find_reserve_range))
		.route(
			"/api/v1/reserve",
			post(create_reservation).delete(delete_reservation),
		)
		.route("/api/v1/reservations/{id}/label", post(label_reservation))
		.layer(tower_http::cors::CorsLayer::permissive())
		.with_state(state)
}
