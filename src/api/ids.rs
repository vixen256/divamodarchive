use crate::AppState;
use crate::models::*;
use axum::{extract::*, http::StatusCode, response::*};
use base64::prelude::*;
use itertools::*;
use serde::{Deserialize, Serialize};
use std::collections::*;
use std::path::Path;
use tokio::process::Command;

#[derive(Serialize, Deserialize)]
pub struct SearchParams {
	pub query: Option<String>,
	pub filter: Option<String>,
	pub limit: Option<usize>,
	pub offset: Option<usize>,
}

#[derive(Serialize, Deserialize)]
pub struct MeilisearchPv {
	pub uid: u64,
	pub post: i32,
	pub pv_id: i32,
	pub song_name: String,
	pub song_name_en: String,
	pub song_info: Option<pv_db::SongInfo>,
	pub song_info_en: Option<pv_db::SongInfo>,
	pub levels: [Option<pv_db::Level>; 5],
}

#[derive(Serialize, Deserialize)]
pub struct MeilisearchModule {
	pub uid: u64,
	pub post_id: i32,
	pub module_id: i32,
	#[serde(flatten)]
	pub module: module_db::Module,
}

#[derive(Serialize, Deserialize)]
pub struct MeilisearchCstmItem {
	pub uid: u64,
	pub post_id: i32,
	pub customize_item_id: i32,
	#[serde(flatten)]
	pub customize_item: module_db::CustomizeItem,
}

#[derive(Serialize, Deserialize)]
struct Config {
	include: Option<Vec<String>>,
}

pub const ROM_DIRS: [&'static str; 31] = [
	"rom",
	"rom_ps4",
	"rom_ps4_dlc",
	"rom_ps4_fix",
	"rom_ps4_patch",
	"rom_steam",
	"rom_steam_cn",
	"rom_steam_dlc",
	"rom_steam_en",
	"rom_steam_fr",
	"rom_steam_ge",
	"rom_steam_it",
	"rom_steam_kr",
	"rom_steam_region",
	"rom_steam_region_cn",
	"rom_steam_region_cn",
	"rom_steam_region_dlc",
	"rom_steam_region_dlc_kr",
	"rom_steam_region_en",
	"rom_steam_region_fr",
	"rom_steam_region_ge",
	"rom_steam_region_kr",
	"rom_steam_region_sp",
	"rom_steam_region_tw",
	"rom_steam_sp",
	"rom_steam_tw",
	"rom_switch",
	"rom_switch_cn",
	"rom_switch_en",
	"rom_switch_kr",
	"rom_switch_tw",
];

pub const DB_PREFIXES: [&'static str; 21] = [
	"mod_",
	"",
	"end_",
	"mdata_",
	"patch2_",
	"patch_",
	"dlc13_",
	"dlc12_",
	"dlc14_",
	"dlc9_",
	"dlc8_",
	"dlc11_",
	"dlc10_",
	"dlc4_",
	"dlc3B_",
	"dlc7_",
	"privilege_",
	"dlc2A_",
	"dlc1_",
	"dlc3A_",
	"dlc2B_",
];

pub async fn extract_post_data(post_id: i32, state: AppState) -> Option<()> {
	let post = Post::get_short(post_id, &state.db).await?;
	if post.post_type == PostType::Plugin
		|| post.post_type == PostType::Cover
		|| post.post_type == PostType::Ui
	{
		return None;
	}

	for file in &post.local_files {
		let file = format!("/pixeldrain/{file}");
		let file = Path::new(&file);
		let extension = file.extension()?.to_str()?;

		let dir = temp_dir::TempDir::new().ok()?;
		let dir = dir.path().to_str()?;

		match extension {
			"zip" => {
				Command::new("unzip")
					.arg(file)
					.arg("-d")
					.arg(dir)
					.output()
					.await
					.ok()?;
				()
			}
			"rar" => {
				Command::new("unrar")
					.arg("x")
					.arg(file)
					.arg(dir)
					.output()
					.await
					.ok()?;
				()
			}
			"7z" => {
				Command::new("7z")
					.arg("x")
					.arg(file)
					.arg(format!("-o{dir}"))
					.output()
					.await
					.ok()?;
				()
			}
			_ => {
				continue;
			}
		}

		for file in walkdir::WalkDir::new(dir).into_iter().filter(|file| {
			if let Ok(file) = &file {
				file.path().ends_with("config.toml")
			} else {
				false
			}
		}) {
			let file = file.ok()?;
			let file = file.path();
			let data = tokio::fs::read_to_string(file).await.ok()?;
			let config: Config = toml::from_str(&data).ok()?;
			let Some(include) = config.include else {
				continue;
			};

			for include in &include {
				for rom in &ROM_DIRS {
					let folder = format!("{}/{include}/{rom}", file.parent()?.to_str()?);
					let path = Path::new(&folder);
					if !path.exists() {
						continue;
					}
					for prefix in &DB_PREFIXES {
						let pv_db = format!("{folder}/{prefix}pv_db.txt");
						let path = Path::new(&pv_db);
						if path.exists() {
							if let Ok(data) = tokio::fs::read_to_string(&path).await {
								parse_pv_db(&data, post_id, state.clone()).await;
							}
						}

						let module_tbl = format!("{folder}/{prefix}gm_module_tbl.farc");
						let module_tbl = Path::new(&module_tbl);
						let customize_item_tbl =
							format!("{folder}/{prefix}gm_customize_item_tbl.farc");
						let customize_item_tbl = Path::new(&customize_item_tbl);
						if module_tbl.exists() || customize_item_tbl.exists() {
							let chritm_prop = format!("{folder}/{prefix}chritm_prop.farc");
							let chritm_prop = Path::new(&chritm_prop);
							let str_array = format!("{folder}/lang2/mod_str_array.toml");
							let str_array = Path::new(&str_array);

							let module_tbl = if module_tbl.exists() {
								Some(module_tbl)
							} else {
								None
							};
							let customize_item_tbl = if customize_item_tbl.exists() {
								Some(customize_item_tbl)
							} else {
								None
							};
							let chritm_prop = if chritm_prop.exists() {
								Some(chritm_prop)
							} else {
								None
							};
							let str_array = if str_array.exists() {
								Some(str_array)
							} else {
								None
							};

							parse_module_db(
								module_tbl,
								customize_item_tbl,
								chritm_prop,
								str_array,
								post_id,
								state.clone(),
							)
							.await;
						}
					}
				}
			}
		}
	}

	optimise_reservations(ReservationType::Song, &state).await;
	optimise_reservations(ReservationType::Module, &state).await;
	optimise_reservations(ReservationType::CstmItem, &state).await;

	Some(())
}

async fn parse_module_db<P: AsRef<Path>>(
	module_tbl: Option<P>,
	customize_item_tbl: Option<P>,
	chritm_prop: Option<P>,
	str_array: Option<P>,
	post_id: i32,
	state: AppState,
) -> Option<()> {
	let module_db =
		module_db::ModuleDb::from_files(module_tbl, customize_item_tbl, chritm_prop, str_array)
			.await?;

	let modules = module_db
		.modules
		.into_iter()
		.map(|(id, module)| MeilisearchModule {
			uid: (post_id as u64) << 32 | (id as u64),
			post_id,
			module_id: id,
			module: module,
		})
		.collect::<Vec<_>>();

	let cstm_items = module_db
		.cstm_items
		.into_iter()
		.map(|(id, cstm_item)| MeilisearchCstmItem {
			uid: (post_id as u64) << 32 | (id as u64),
			post_id,
			customize_item_id: id,
			customize_item: cstm_item,
		})
		.collect::<Vec<_>>();

	let base = meilisearch_sdk::search::SearchQuery::new(&state.meilisearch.index("modules"))
		.with_filter("post_id=-1")
		.with_limit(2000)
		.execute::<MeilisearchModule>()
		.await
		.ok()?;

	let mut modules = modules
		.into_iter()
		.filter(|module| {
			!base.hits.iter().any(|base| {
				base.result.module_id == module.module_id
					&& base.result.module.name_jp == module.module.name_jp
			})
		})
		.collect::<Vec<_>>();

	for module in &mut modules {
		for item in &mut module.module.cos.items {
			if item.objset.is_empty() {
				for base_module in &base.hits {
					if base_module.result.module.chara != module.module.chara {
						continue;
					}
					for base_item in &base_module.result.module.cos.items {
						if base_item.id == item.id {
							*item = base_item.clone();
						}
					}
				}
			}
		}
	}

	state
		.meilisearch
		.index("modules")
		.add_or_update(&modules, Some("uid"))
		.await
		.ok()?;

	let base = meilisearch_sdk::search::SearchQuery::new(&state.meilisearch.index("cstm_items"))
		.with_filter("post_id=-1")
		.with_limit(2000)
		.execute::<MeilisearchCstmItem>()
		.await
		.ok()?;

	let cstm_items = cstm_items
		.into_iter()
		.filter(|cstm_item| {
			!base.hits.iter().any(|base| {
				base.result.customize_item_id == cstm_item.customize_item_id
					&& base.result.customize_item.name_jp == cstm_item.customize_item.name_jp
			})
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("cstm_items")
		.add_or_update(&cstm_items, Some("uid"))
		.await
		.ok()?;

	Some(())
}

async fn parse_pv_db(data: &str, post_id: i32, state: AppState) -> Option<()> {
	let pv_db = pv_db::PvDb::from_str(data)?;

	let mut documents = Vec::new();
	for (id, entry) in pv_db.pvs.iter() {
		let mut levels = [const { None }; 5];
		if let Some(difficulties) = &entry.difficulty {
			if let Some(easys) = &difficulties.easy {
				for easy in easys {
					if easy.edition == Some(0) {
						levels[0] = easy.level.clone();
					}
				}
			}
			if let Some(normals) = &difficulties.normal {
				for normal in normals {
					if normal.edition == Some(0) {
						levels[1] = normal.level.clone();
					}
				}
			}
			if let Some(hards) = &difficulties.hard {
				for hard in hards {
					if hard.edition == Some(0) {
						levels[2] = hard.level.clone();
					}
				}
			}
			if let Some(extremes) = &difficulties.extreme {
				for extreme in extremes {
					if extreme.edition == Some(0) {
						levels[3] = extreme.level.clone();
					} else if extreme.edition == Some(1) {
						levels[4] = extreme.level.clone();
					}
				}
			}
		}
		documents.push(MeilisearchPv {
			uid: (post_id as u64) << 32 | (*id as u64),
			post: post_id,
			pv_id: *id as i32,
			song_name: entry.song_name.clone(),
			song_name_en: entry.song_name_en.clone(),
			song_info: entry.songinfo.clone(),
			song_info_en: entry.songinfo_en.clone(),
			levels,
		});
	}

	let base = meilisearch_sdk::search::SearchQuery::new(&state.meilisearch.index("pvs"))
		.with_filter("post=-1")
		.with_limit(300)
		.execute::<MeilisearchPv>()
		.await
		.unwrap();

	let pvs = documents
		.into_iter()
		.filter(|pv| {
			!base.hits.iter().any(|base| {
				base.result.pv_id == pv.pv_id
					&& base.result.song_name == pv.song_name
					&& base.result.song_name_en == pv.song_name_en
			})
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("pvs")
		.add_or_update(&pvs, Some("uid"))
		.await
		.unwrap();

	Some(())
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Pv {
	pub uid: String,
	pub post: Option<i32>,
	pub id: i32,
	pub name: String,
	pub name_en: String,
	pub song_info: Option<pv_db::SongInfo>,
	pub song_info_en: Option<pv_db::SongInfo>,
	pub levels: [Option<pv_db::Level>; 5],
}

impl Pv {
	pub fn has_music(&self) -> bool {
		if let Some(song_info) = &self.song_info {
			if let Some(music) = &song_info.music {
				if !music.trim().is_empty() {
					return true;
				}
			}
		}
		if let Some(song_info) = &self.song_info_en {
			if let Some(music) = &song_info.music {
				if !music.trim().is_empty() {
					return true;
				}
			}
		}
		return false;
	}

	pub fn has_lyrics(&self) -> bool {
		if let Some(song_info) = &self.song_info {
			if let Some(lyrics) = &song_info.lyrics {
				if !lyrics.trim().is_empty() {
					return true;
				}
			}
		}
		if let Some(song_info) = &self.song_info_en {
			if let Some(lyrics) = &song_info.lyrics {
				if !lyrics.trim().is_empty() {
					return true;
				}
			}
		}
		return false;
	}

	pub fn has_arranger(&self) -> bool {
		if let Some(song_info) = &self.song_info {
			if let Some(arranger) = &song_info.arranger {
				if !arranger.trim().is_empty() {
					return true;
				}
			}
		}
		if let Some(song_info) = &self.song_info_en {
			if let Some(arranger) = &song_info.arranger {
				if !arranger.trim().is_empty() {
					return true;
				}
			}
		}
		return false;
	}

	pub fn has_manipulator(&self) -> bool {
		if let Some(song_info) = &self.song_info {
			if let Some(manipulator) = &song_info.manipulator {
				if manipulator.trim().is_empty() {
					return true;
				}
			}
		}
		if let Some(song_info) = &self.song_info_en {
			if let Some(manipulator) = &song_info.manipulator {
				if !manipulator.trim().is_empty() {
					return true;
				}
			}
		}
		return false;
	}

	pub fn has_editor(&self) -> bool {
		if let Some(song_info) = &self.song_info {
			if let Some(pv_editor) = &song_info.pv_editor {
				if !pv_editor.trim().is_empty() {
					return true;
				}
			}
		}
		if let Some(song_info) = &self.song_info_en {
			if let Some(pv_editor) = &song_info.pv_editor {
				if !pv_editor.trim().is_empty() {
					return true;
				}
			}
		}
		return false;
	}

	pub fn has_guitar(&self) -> bool {
		if let Some(song_info) = &self.song_info {
			if let Some(guitar_player) = &song_info.guitar_player {
				if !guitar_player.trim().is_empty() {
					return true;
				}
			}
		}
		if let Some(song_info) = &self.song_info_en {
			if let Some(guitar_player) = &song_info.guitar_player {
				if !guitar_player.trim().is_empty() {
					return true;
				}
			}
		}
		return false;
	}

	pub fn song_info_count(&self) -> isize {
		self.has_music() as isize
			+ self.has_lyrics() as isize
			+ self.has_arranger() as isize
			+ self.has_manipulator() as isize
			+ self.has_editor() as isize
			+ self.has_guitar() as isize
	}
}

#[derive(Serialize, Deserialize)]
pub struct Module {
	pub uid: String,
	pub post: Option<i32>,
	pub id: i32,
	pub module: module_db::Module,
}

#[derive(Serialize, Deserialize)]
pub struct CstmItem {
	pub uid: String,
	pub post: Option<i32>,
	pub id: i32,
	pub cstm_item: module_db::CustomizeItem,
}

#[derive(Serialize, Deserialize, Default)]
pub struct PvSearch {
	pub pvs: Vec<Pv>,
	pub posts: BTreeMap<i32, Post>,
}

pub async fn search_pvs(
	axum_extra::extract::Query(query): axum_extra::extract::Query<SearchParams>,
	State(state): State<AppState>,
) -> Result<Json<PvSearch>, (StatusCode, String)> {
	let index = state.meilisearch.index("pvs");
	let mut search = meilisearch_sdk::search::SearchQuery::new(&index);

	search.query = query.query.as_ref().map(|query| query.as_str());

	search.limit = query.limit;
	search.offset = query.offset;

	search.sort = Some(&["pv_id:asc"]);

	let filter = if let Some(filter) = &query.filter {
		format!("{filter}")
	} else {
		String::new()
	};

	search.filter = Some(meilisearch_sdk::search::Filter::new(sqlx::Either::Left(
		filter.as_str(),
	)));

	let pvs = search
		.execute::<MeilisearchPv>()
		.await
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

	let pvs = pvs.hits.into_iter().map(|p| p.result).collect::<Vec<_>>();

	let mut vec = Vec::with_capacity(pvs.len());
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();
	for pv in pvs {
		let post = if pv.post == -1 {
			None
		} else if let Some(post) = posts.get(&pv.post) {
			Some(post.id)
		} else if let Some(mut post) = Post::get_full(pv.post, &state.db).await {
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
			posts.insert(post.id, post.clone());
			Some(post.id)
		} else if pv.post != -1 {
			let pvs = state.meilisearch.index("pvs");
			_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&pvs)
				.with_filter(&format!("post={}", pv.post))
				.execute::<crate::api::ids::MeilisearchPv>()
				.await;
			None
		} else {
			None
		};

		vec.push(Pv {
			uid: BASE64_STANDARD.encode(pv.uid.to_ne_bytes()),
			id: pv.pv_id,
			name: pv.song_name,
			name_en: pv.song_name_en,
			song_info: pv.song_info,
			song_info_en: pv.song_info_en,
			levels: pv.levels,
			post,
		})
	}

	Ok(Json(PvSearch { pvs: vec, posts }))
}

#[derive(Serialize, Deserialize, Default)]
pub struct ModuleSearch {
	pub modules: Vec<Module>,
	pub posts: BTreeMap<i32, Post>,
}

pub async fn search_modules(
	axum_extra::extract::Query(query): axum_extra::extract::Query<SearchParams>,
	State(state): State<AppState>,
) -> Result<Json<ModuleSearch>, (StatusCode, String)> {
	let index = state.meilisearch.index("modules");
	let mut search = meilisearch_sdk::search::SearchQuery::new(&index);

	search.query = query.query.as_ref().map(|query| query.as_str());

	search.limit = query.limit;
	search.offset = query.offset;

	search.sort = Some(&["module_id:asc"]);

	let filter = if let Some(filter) = &query.filter {
		format!("{filter}")
	} else {
		String::new()
	};

	search.filter = Some(meilisearch_sdk::search::Filter::new(sqlx::Either::Left(
		filter.as_str(),
	)));

	let modules = search
		.execute::<MeilisearchModule>()
		.await
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

	let modules = modules
		.hits
		.into_iter()
		.map(|module| module.result)
		.collect::<Vec<_>>();

	let mut vec = Vec::with_capacity(modules.len());
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();
	for module in modules {
		let post = if module.post_id == -1 {
			None
		} else if let Some(post) = posts.get(&module.post_id) {
			Some(post.id)
		} else if let Some(mut post) = Post::get_full(module.post_id, &state.db).await {
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
			posts.insert(post.id, post.clone());
			Some(post.id)
		} else if module.post_id != -1 {
			let modules = state.meilisearch.index("modules");
			_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&modules)
				.with_filter(&format!("post_id={}", module.post_id))
				.execute::<crate::api::ids::MeilisearchModule>()
				.await;
			None
		} else {
			None
		};

		vec.push(Module {
			uid: BASE64_STANDARD.encode(module.uid.to_ne_bytes()),
			post,
			id: module.module_id,
			module: module.module,
		})
	}

	Ok(Json(ModuleSearch {
		modules: vec,
		posts,
	}))
}

#[derive(Serialize, Deserialize, Default)]
pub struct CstmItemSearch {
	pub cstm_items: Vec<CstmItem>,
	pub bound_modules: BTreeMap<i32, Module>,
	pub posts: BTreeMap<i32, Post>,
}

pub async fn search_cstm_items(
	axum_extra::extract::Query(query): axum_extra::extract::Query<SearchParams>,
	State(state): State<AppState>,
) -> Result<Json<CstmItemSearch>, (StatusCode, String)> {
	let index = state.meilisearch.index("cstm_items");
	let mut search = meilisearch_sdk::search::SearchQuery::new(&index);

	search.query = query.query.as_ref().map(|query| query.as_str());

	search.limit = query.limit;
	search.offset = query.offset;

	search.sort = Some(&["customize_item_id:asc"]);

	let filter = if let Some(filter) = &query.filter {
		format!("{filter}")
	} else {
		String::new()
	};

	search.filter = Some(meilisearch_sdk::search::Filter::new(sqlx::Either::Left(
		filter.as_str(),
	)));

	let cstm_items = search
		.execute::<MeilisearchCstmItem>()
		.await
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

	let cstm_items = cstm_items
		.hits
		.into_iter()
		.map(|cstm_item| cstm_item.result)
		.collect::<Vec<_>>();

	let mut vec = Vec::with_capacity(cstm_items.len());
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();
	let mut pending_bound_modules: BTreeSet<(i32, Option<i32>)> = BTreeSet::new();

	for cstm_item in cstm_items {
		let post = if cstm_item.post_id == -1 {
			None
		} else if let Some(post) = posts.get(&cstm_item.post_id) {
			Some(post.id)
		} else if let Some(mut post) = Post::get_full(cstm_item.post_id, &state.db).await {
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
			posts.insert(post.id, post.clone());
			Some(post.id)
		} else if cstm_item.post_id != -1 {
			let cstm_items = state.meilisearch.index("cstm_items");
			_ = meilisearch_sdk::documents::DocumentDeletionQuery::new(&cstm_items)
				.with_filter(&format!("post_id={}", cstm_item.post_id))
				.execute::<crate::api::ids::MeilisearchCstmItem>()
				.await;
			None
		} else {
			None
		};

		if let Some(bind_module) = cstm_item.customize_item.bind_module {
			if bind_module != -1 {
				pending_bound_modules.insert((bind_module, post));
			}
		}

		let customize_item = if cstm_item.customize_item.bind_module == Some(-1) {
			module_db::CustomizeItem {
				bind_module: None,
				chara: cstm_item.customize_item.chara,
				part: cstm_item.customize_item.part,
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

		vec.push(CstmItem {
			uid: BASE64_STANDARD.encode(cstm_item.uid.to_ne_bytes()),
			post,
			id: cstm_item.customize_item_id,
			cstm_item: customize_item,
		})
	}

	let mut bound_modules = BTreeMap::new();

	if pending_bound_modules.len() > 0 {
		let filter = pending_bound_modules
			.iter()
			.map(|(module, post)| {
				if let Some(post) = post {
					format!("(module_id={module} AND (post_id={post} OR post_id=-1))")
				} else {
					format!("(module_id={module} AND post_id=-1)")
				}
			})
			.intersperse(String::from(" OR "))
			.collect::<String>();

		let Json(modules) = crate::api::ids::search_modules(
			axum_extra::extract::Query(crate::api::ids::SearchParams {
				query: None,
				filter: Some(filter),
				limit: Some(pending_bound_modules.len()),
				offset: Some(0),
			}),
			State(state.clone()),
		)
		.await
		.unwrap_or_default();

		for module in modules.modules {
			if let Some(post_id) = &module.post {
				if let Some(post) = modules.posts.get(post_id) {
					if !posts.contains_key(post_id) {
						posts.insert(*post_id, post.clone());
					}
				}
			}

			bound_modules.insert(
				module.id,
				Module {
					uid: module.uid,
					post: module.post,
					id: module.id,
					module: module.module,
				},
			);
		}
	}

	Ok(Json(CstmItemSearch {
		cstm_items: vec,
		bound_modules,
		posts,
	}))
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[repr(i32)]
pub enum ReservationType {
	Song = 0,
	Module = 1,
	CstmItem = 2,
}

impl From<i32> for ReservationType {
	fn from(value: i32) -> Self {
		match value {
			1 => Self::Module,
			2 => Self::CstmItem,
			_ => Self::Song,
		}
	}
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Reservation {
	pub user: User,
	pub reservation_type: ReservationType,
	pub range_start: i32,
	pub length: i32,
	#[serde(with = "time::serde::rfc3339")]
	pub time: time::OffsetDateTime,
}

#[derive(Serialize, Deserialize, PartialEq)]
pub enum ReserveRangeResult {
	ValidRange,                  // Completly empty
	PartialValidRange(Vec<i32>), // Range contains content that the user is an author of, returns which ids those are
	InvalidRange,                // Range containts content that the user is not an author
	InvalidLength(i32),          // The length is too high, returns the users max remaining length
	InvalidAlignment(u32),       // Start was improperly aligned
}

#[derive(Serialize, Deserialize)]
pub struct ReserveRangeArgs {
	pub reservation_type: ReservationType,
	pub start: u32,
	pub length: u32,
}

pub async fn create_reservation(
	user: User,
	State(state): State<AppState>,
	Json(query): Json<ReserveRangeArgs>,
) -> Json<ReserveRangeResult> {
	if query.reservation_type != ReservationType::Song || query.start == 0 || query.length == 0 {
		return Json(ReserveRangeResult::InvalidRange);
	}

	let validity = check_reserve_range(
		query.reservation_type,
		query.start,
		query.length,
		&user,
		&state,
	)
	.await;

	match validity {
		ReserveRangeResult::ValidRange => {
			let now = time::OffsetDateTime::now_utc();
			let time = time::PrimitiveDateTime::new(now.date(), now.time());
			_ = sqlx::query!(
				"INSERT INTO reservations VALUES($1, $2, $3, $4, $5)",
				user.id,
				query.reservation_type as i32,
				query.start as i32,
				query.length as i32,
				time
			)
			.execute(&state.db)
			.await;
		}
		ReserveRangeResult::PartialValidRange(ref old_ids) => {
			let old_ids = old_ids.iter().cloned().collect::<BTreeSet<_>>();
			let new_ids = (query.start as i32..(query.start as i32 + query.length as i32))
				.collect::<BTreeSet<_>>();

			let time = time::OffsetDateTime::now_utc();

			let mut ranges: Vec<Reservation> = Vec::new();
			for id in new_ids.difference(&old_ids) {
				if let Some(last) = ranges.last_mut() {
					if last.range_start + last.length == *id {
						last.length += 1;
					} else {
						ranges.push(Reservation {
							user: user.clone(),
							reservation_type: query.reservation_type,
							range_start: *id,
							length: 1,
							time,
						});
					}
				} else {
					ranges.push(Reservation {
						user: user.clone(),
						reservation_type: query.reservation_type,
						range_start: *id,
						length: 1,
						time,
					});
				}
			}

			for reservation in ranges {
				_ = sqlx::query!(
					"INSERT INTO reservations VALUES($1, $2, $3, $4, $5)",
					reservation.user.id,
					reservation.reservation_type as i32,
					reservation.range_start,
					reservation.length,
					time::PrimitiveDateTime::new(reservation.time.date(), reservation.time.time()),
				)
				.execute(&state.db)
				.await;
			}

			optimise_reservations(query.reservation_type, &state).await;
		}
		_ => {}
	}

	Json(validity)
}

pub async fn delete_reservation(
	user: User,
	State(state): State<AppState>,
	Json(query): Json<ReserveRangeArgs>,
) {
	if query.start == 0 || query.length == 0 {
		return;
	}

	let reservered_ids = sqlx::query!(
		r#"
		SELECT * FROM reservations r
		LEFT JOIN users u ON r.user_id = u.id
		WHERE r.reservation_type = $1
		AND r.user_id = $2
		AND (r.range_start >= $3 OR r.range_start + r.length > $3) AND r.range_start < $4
		"#,
		query.reservation_type as i32,
		user.id,
		query.start as i32,
		(query.start + query.length) as i32
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.flat_map(|reservation| {
		(reservation.range_start..(reservation.range_start + reservation.length))
			.map(|id| (id, reservation.time.assume_offset(time::UtcOffset::UTC)))
	})
	.collect::<BTreeMap<_, _>>();

	let ids =
		(query.start as i32..(query.start as i32 + query.length as i32)).collect::<BTreeSet<_>>();

	if ids.len() > reservered_ids.len() {
		return;
	}

	let mut ranges: Vec<Reservation> = Vec::new();
	for id in reservered_ids
		.keys()
		.cloned()
		.collect::<BTreeSet<_>>()
		.difference(&ids)
	{
		if let Some(last) = ranges.last_mut() {
			if last.range_start + last.length == *id {
				last.length += 1;
				if reservered_ids[id] > last.time {
					last.time = reservered_ids[id];
				}
			} else {
				ranges.push(Reservation {
					user: user.clone(),
					reservation_type: query.reservation_type,
					range_start: *id,
					length: 1,
					time: reservered_ids[id],
				});
			}
		} else {
			ranges.push(Reservation {
				user: user.clone(),
				reservation_type: query.reservation_type,
				range_start: *id,
				length: 1,
				time: reservered_ids[id],
			});
		}
	}

	_ = sqlx::query!(
		r#"
		UPDATE reservations r
		SET time = '2000-01-01'
		WHERE r.reservation_type = $1
		AND r.user_id = $2
		AND (r.range_start >= $3 OR r.range_start + r.length > $3) AND r.range_start < $4
		"#,
		query.reservation_type as i32,
		user.id,
		query.start as i32,
		(query.start + query.length) as i32
	)
	.execute(&state.db)
	.await;

	for reservation in ranges {
		_ = sqlx::query!(
			"INSERT INTO reservations VALUES($1, $2, $3, $4, $5)",
			reservation.user.id,
			reservation.reservation_type as i32,
			reservation.range_start,
			reservation.length,
			time::PrimitiveDateTime::new(reservation.time.date(), reservation.time.time()),
		)
		.execute(&state.db)
		.await;
	}

	_ = sqlx::query!(
		r#"
		DELETE FROM reservations r
		WHERE time = '2000-01-01'
		AND r.reservation_type = $1
		AND r.user_id = $2
		AND (r.range_start >= $3 OR r.range_start + r.length > $3) AND r.range_start < $4
		"#,
		query.reservation_type as i32,
		user.id,
		query.start as i32,
		(query.start + query.length) as i32
	)
	.execute(&state.db)
	.await;
}

pub async fn web_check_reserve_range(
	axum_extra::extract::Query(query): axum_extra::extract::Query<ReserveRangeArgs>,
	user: User,
	State(state): State<AppState>,
) -> Json<ReserveRangeResult> {
	if query.start == 0 || query.length == 0 {
		return Json(ReserveRangeResult::InvalidRange);
	}

	Json(
		check_reserve_range(
			query.reservation_type,
			query.start,
			query.length,
			&user,
			&state,
		)
		.await,
	)
}

/*
- Must be aligned, e.g. less than 10 means no alignment, 10+ means the first id must be aligned to 10 and end with `0`, 100+ means the first id must be aligned to 100 and end with `00`
- Can go through mods the user is an author of
- Max number of reserved ids is 30 + half of how many items the user has already uploaded rounded up to the nearest multiple of 10, e.g. if a user has uploaded a song pack with 30 songs they can reserve 50 song ids and 30 module/cstm_item ids
*/

pub async fn check_reserve_range(
	reservation_type: ReservationType,
	start: u32,
	length: u32,
	user: &User,
	state: &AppState,
) -> ReserveRangeResult {
	let max = get_user_max_reservations(reservation_type, &user, &state).await;
	if max < length as i32 {
		return ReserveRangeResult::InvalidLength(max);
	}

	let alignment = 10_u32
		.checked_pow(length.checked_ilog10().unwrap_or(0))
		.unwrap_or(1);
	if start % alignment != 0 {
		return ReserveRangeResult::InvalidAlignment(alignment);
	}

	let conflicts = match reservation_type {
		ReservationType::Song => {
			let index = state.meilisearch.index("pvs");

			let filter = (start..(start + length))
				.map(|id| format!("pv_id={id}"))
				.intersperse(String::from(" OR "))
				.collect::<String>();

			let search = meilisearch_sdk::search::SearchQuery::new(&index)
				.with_limit(10000)
				.with_sort(&["pv_id:asc"])
				.with_filter(&filter)
				.execute::<MeilisearchPv>()
				.await;

			search.map_or(Vec::new(), |search| {
				search
					.hits
					.into_iter()
					.map(|pv| (pv.result.pv_id, pv.result.post))
					.collect::<Vec<_>>()
			})
		}
		ReservationType::Module => {
			let index = state.meilisearch.index("modules");

			let filter = (start..(start + length))
				.map(|id| format!("module_id={id}"))
				.intersperse(String::from(" OR "))
				.collect::<String>();

			let search = meilisearch_sdk::search::SearchQuery::new(&index)
				.with_limit(10000)
				.with_sort(&["module_id:asc"])
				.with_filter(&filter)
				.execute::<MeilisearchModule>()
				.await;

			search.map_or(Vec::new(), |search| {
				search
					.hits
					.into_iter()
					.map(|module| (module.result.module_id, module.result.post_id))
					.collect::<Vec<_>>()
			})
		}
		ReservationType::CstmItem => {
			let index = state.meilisearch.index("cstm_items");

			let filter = (start..(start + length))
				.map(|id| format!("customize_item_id={id}"))
				.intersperse(String::from(" OR "))
				.collect::<String>();

			let search = meilisearch_sdk::search::SearchQuery::new(&index)
				.with_limit(10000)
				.with_sort(&["customize_item_id:asc"])
				.with_filter(&filter)
				.execute::<MeilisearchCstmItem>()
				.await;

			search.map_or(Vec::new(), |search| {
				search
					.hits
					.into_iter()
					.map(|cstm_item| (cstm_item.result.customize_item_id, cstm_item.result.post_id))
					.collect::<Vec<_>>()
			})
		}
	};

	let mut partial_range = Vec::new();
	for (id, post) in conflicts {
		if post == -1 {
			return ReserveRangeResult::InvalidRange;
		}
		let Some(post) = Post::get_short(post, &state.db).await else {
			continue;
		};
		if post.authors.contains(&user) {
			partial_range.push(id);
		} else {
			return ReserveRangeResult::InvalidRange;
		}
	}

	let conflicts = sqlx::query!(
		"SELECT u.id, r.range_start, r.length FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE (r.range_start >= $1 OR r.range_start + r.length > $1) AND r.range_start < $2",
		start as i32,
		(start + length) as i32
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default();

	for conflict in conflicts {
		if conflict.id == user.id {
			for conflict in conflict.range_start..(conflict.range_start + conflict.length) {
				if conflict >= start as i32 && conflict < (start + length) as i32 {
					partial_range.push(conflict);
				}
			}
		} else {
			return ReserveRangeResult::InvalidRange;
		}
	}

	if partial_range == ((start as i32)..(start as i32 + length as i32)).collect::<Vec<_>>() {
		return ReserveRangeResult::InvalidRange;
	}

	if partial_range.len() > 0 {
		return ReserveRangeResult::PartialValidRange(partial_range);
	}

	ReserveRangeResult::ValidRange
}

pub async fn get_user_max_reservations(
	reservation_type: ReservationType,
	user: &User,
	state: &AppState,
) -> i32 {
	let existing_reservations = sqlx::query!(
		"SELECT range_start, length FROM reservations WHERE reservation_type = $1 AND user_id = $2 AND time != '1970-01-01'",
		reservation_type as i32,
		user.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default();

	let user_posts = sqlx::query!(
		"SELECT post_id FROM post_authors WHERE user_id = $1",
		user.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default();

	let ids = if user_posts.len() > 0 {
		match reservation_type {
			ReservationType::Song => {
				let index = state.meilisearch.index("pvs");

				let filter = user_posts
					.iter()
					.map(|post| format!("post={}", post.post_id))
					.intersperse(String::from(" OR "))
					.collect::<String>();

				let search = meilisearch_sdk::search::SearchQuery::new(&index)
					.with_limit(10000)
					.with_sort(&["pv_id:asc"])
					.with_filter(&filter)
					.execute::<MeilisearchPv>()
					.await;

				search.map_or(BTreeSet::new(), |search| {
					search
						.hits
						.into_iter()
						.map(|pv| pv.result.pv_id)
						.collect::<BTreeSet<_>>()
				})
			}
			ReservationType::Module => {
				let index = state.meilisearch.index("modules");

				let filter = user_posts
					.iter()
					.map(|post| format!("post_id={}", post.post_id))
					.intersperse(String::from(" OR "))
					.collect::<String>();

				let search = meilisearch_sdk::search::SearchQuery::new(&index)
					.with_limit(10000)
					.with_sort(&["module_id:asc"])
					.with_filter(&filter)
					.execute::<MeilisearchModule>()
					.await;

				search.map_or(BTreeSet::new(), |search| {
					search
						.hits
						.into_iter()
						.map(|module| module.result.module_id)
						.collect::<BTreeSet<_>>()
				})
			}
			ReservationType::CstmItem => {
				let index = state.meilisearch.index("cstm_items");

				let filter = user_posts
					.iter()
					.map(|post| format!("post_id={}", post.post_id))
					.intersperse(String::from(" OR "))
					.collect::<String>();

				let search = meilisearch_sdk::search::SearchQuery::new(&index)
					.with_limit(10000)
					.with_sort(&["customize_item_id:asc"])
					.with_filter(&filter)
					.execute::<MeilisearchCstmItem>()
					.await;

				search.map_or(BTreeSet::new(), |search| {
					search
						.hits
						.into_iter()
						.map(|cstm_item| cstm_item.result.customize_item_id)
						.collect::<BTreeSet<_>>()
				})
			}
		}
	} else {
		BTreeSet::new()
	};

	let existing_reservations = existing_reservations
		.iter()
		.flat_map(|reservation| {
			(reservation.range_start)..(reservation.range_start + reservation.length)
		})
		.filter(|reservation| !ids.contains(reservation))
		.count();

	30 + (ids.len() / 2).next_multiple_of(10) as i32 - existing_reservations as i32
}

pub async fn web_find_reserve_range(
	axum_extra::extract::Query(query): axum_extra::extract::Query<ReserveRangeArgs>,
	user: User,
	State(state): State<AppState>,
) -> Json<i32> {
	if query.start != 0 || query.length == 0 {
		return Json(0);
	}

	Json(find_reservable_range(query.reservation_type, query.length, &user, &state).await)
}

pub async fn find_reservable_range(
	reservation_type: ReservationType,
	length: u32,
	user: &User,
	state: &AppState,
) -> i32 {
	let max = get_user_max_reservations(reservation_type, &user, &state).await;
	if max < length as i32 {
		return -1;
	}

	let mut reservations = sqlx::query!(
		"SELECT range_start, length FROM reservations WHERE reservation_type = $1",
		reservation_type as i32,
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	.iter()
	.flat_map(|reservation| {
		(reservation.range_start as u32)
			..(reservation.range_start as u32 + reservation.length as u32)
	})
	.collect::<BTreeSet<_>>();

	let mut ids = match reservation_type {
		ReservationType::Song => {
			let index = state.meilisearch.index("pvs");

			let search = meilisearch_sdk::search::SearchQuery::new(&index)
				.with_limit(100000)
				.with_sort(&["pv_id:asc"])
				.execute::<MeilisearchPv>()
				.await;

			search.map_or(BTreeSet::new(), |search| {
				search
					.hits
					.into_iter()
					.map(|pv| pv.result.pv_id as u32)
					.collect::<BTreeSet<_>>()
			})
		}
		ReservationType::Module => {
			let index = state.meilisearch.index("modules");

			let search = meilisearch_sdk::search::SearchQuery::new(&index)
				.with_limit(100000)
				.with_sort(&["module_id:asc"])
				.execute::<MeilisearchModule>()
				.await;

			search.map_or(BTreeSet::new(), |search| {
				search
					.hits
					.into_iter()
					.map(|module| module.result.module_id as u32)
					.collect::<BTreeSet<_>>()
			})
		}
		ReservationType::CstmItem => {
			let index = state.meilisearch.index("cstm_items");

			let search = meilisearch_sdk::search::SearchQuery::new(&index)
				.with_limit(100000)
				.with_sort(&["customize_item_id:asc"])
				.execute::<MeilisearchCstmItem>()
				.await;

			search.map_or(BTreeSet::new(), |search| {
				search
					.hits
					.into_iter()
					.map(|cstm_item| cstm_item.result.customize_item_id as u32)
					.collect::<BTreeSet<_>>()
			})
		}
	};

	ids.append(&mut reservations);

	let alignment = 10_u32
		.checked_pow(length.checked_ilog10().unwrap_or(0))
		.unwrap_or(1);

	for (id, next) in ids.iter().tuple_windows() {
		let res = (id + 1).next_multiple_of(alignment);
		if res + length <= *next {
			return res as i32;
		}
	}

	ids.last()
		.map_or(0, |id| id + 1)
		.next_multiple_of(alignment) as i32
}

pub async fn optimise_reservations(reservation_type: ReservationType, state: &AppState) {
	let users = sqlx::query_as!(
		User,
		r#"
		SELECT DISTINCT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme
		FROM reservations r
		LEFT JOIN users u ON r.user_id = u.id
		WHERE r.reservation_type = $1
		"#,
		reservation_type as i32
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default();

	for user in users {
		let reservered_ids = sqlx::query!(
			r#"
			SELECT * FROM reservations r
			LEFT JOIN users u ON r.user_id = u.id
			WHERE r.reservation_type = $1
			AND r.user_id = $2
			"#,
			reservation_type as i32,
			user.id,
		)
		.fetch_all(&state.db)
		.await
		.unwrap_or_default()
		.iter()
		.flat_map(|reservation| {
			(reservation.range_start..(reservation.range_start + reservation.length))
				.map(|id| (id, reservation.time.assume_offset(time::UtcOffset::UTC)))
		})
		.collect::<BTreeMap<_, _>>();

		let user_posts = sqlx::query!(
			r#"
			SELECT post_id
			FROM post_authors
			WHERE user_id = $1
			"#,
			user.id,
		)
		.fetch_all(&state.db)
		.await
		.unwrap_or_default();

		let ids = if user_posts.len() > 0 {
			match reservation_type {
				ReservationType::Song => {
					let index = state.meilisearch.index("pvs");

					let filter = user_posts
						.iter()
						.map(|post| format!("post={}", post.post_id))
						.intersperse(String::from(" OR "))
						.collect::<String>();

					let search = meilisearch_sdk::search::SearchQuery::new(&index)
						.with_limit(10000)
						.with_sort(&["pv_id:asc"])
						.with_filter(&filter)
						.execute::<MeilisearchPv>()
						.await;

					search.map_or(BTreeSet::new(), |search| {
						search
							.hits
							.into_iter()
							.map(|pv| pv.result.pv_id)
							.collect::<BTreeSet<_>>()
					})
				}
				ReservationType::Module => {
					let index = state.meilisearch.index("modules");

					let filter = user_posts
						.iter()
						.map(|post| format!("post_id={}", post.post_id))
						.intersperse(String::from(" OR "))
						.collect::<String>();

					let search = meilisearch_sdk::search::SearchQuery::new(&index)
						.with_limit(10000)
						.with_sort(&["module_id:asc"])
						.with_filter(&filter)
						.execute::<MeilisearchModule>()
						.await;

					search.map_or(BTreeSet::new(), |search| {
						search
							.hits
							.into_iter()
							.map(|module| module.result.module_id)
							.collect::<BTreeSet<_>>()
					})
				}
				ReservationType::CstmItem => {
					let index = state.meilisearch.index("cstm_items");

					let filter = user_posts
						.iter()
						.map(|post| format!("post_id={}", post.post_id))
						.intersperse(String::from(" OR "))
						.collect::<String>();

					let search = meilisearch_sdk::search::SearchQuery::new(&index)
						.with_limit(10000)
						.with_sort(&["customize_item_id:asc"])
						.with_filter(&filter)
						.execute::<MeilisearchCstmItem>()
						.await;

					search.map_or(BTreeSet::new(), |search| {
						search
							.hits
							.into_iter()
							.map(|cstm_item| cstm_item.result.customize_item_id)
							.collect::<BTreeSet<_>>()
					})
				}
			}
		} else {
			BTreeSet::new()
		};

		let mut ranges: Vec<Reservation> = Vec::new();
		for id in reservered_ids
			.keys()
			.cloned()
			.collect::<BTreeSet<_>>()
			.difference(&ids)
		{
			if let Some(last) = ranges.last_mut() {
				if last.range_start + last.length == *id {
					last.length += 1;
					if reservered_ids[id] > last.time {
						last.time = reservered_ids[id];
					}
				} else {
					ranges.push(Reservation {
						user: user.clone(),
						reservation_type,
						range_start: *id,
						length: 1,
						time: reservered_ids[id],
					});
				}
			} else {
				ranges.push(Reservation {
					user: user.clone(),
					reservation_type,
					range_start: *id,
					length: 1,
					time: reservered_ids[id],
				});
			}
		}

		_ = sqlx::query!(
			r#"
			UPDATE reservations r
			SET time = '2000-01-01'
			WHERE r.reservation_type = $1
			AND r.user_id = $2
			"#,
			reservation_type as i32,
			user.id,
		)
		.execute(&state.db)
		.await;

		for reservation in ranges {
			_ = sqlx::query!(
				"INSERT INTO reservations VALUES($1, $2, $3, $4, $5)",
				reservation.user.id,
				reservation.reservation_type as i32,
				reservation.range_start,
				reservation.length,
				time::PrimitiveDateTime::new(reservation.time.date(), reservation.time.time()),
			)
			.execute(&state.db)
			.await;
		}

		_ = sqlx::query!(
			r#"
			DELETE FROM reservations r
			WHERE time = '2000-01-01'
			AND r.reservation_type = $1
			AND r.user_id = $2
			"#,
			reservation_type as i32,
			user.id,
		)
		.execute(&state.db)
		.await;
	}
}
