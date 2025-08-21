use crate::AppState;
use crate::models::*;
use axum::{extract::*, http::StatusCode, response::*};
use base64::prelude::*;
use itertools::*;
use serde::{Deserialize, Serialize};
use std::collections::*;
use std::path::Path;
use tokio::process::Command;
use utoipa::{IntoParams, ToSchema};

#[derive(Serialize, Deserialize, Clone, IntoParams)]
pub struct SearchParams {
	pub query: Option<String>,
	/**
	A meilisearch filter, such as `post != 100`
	Attributes depend on search
	PVs: post, pv_id
	Modules: post_id, module_id, chara, cos.id, cos.items.id
	Cstm_items: post_id, customize_item_id
	NC Songs: post_id, pv_id
	Sprites/Aets/Objsets/Textures: post_id, id, name
	*/
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
pub struct MeilisearchNcSong {
	pub uid: u64,
	pub post_id: i32,
	pub pv_id: i32,
	pub difficulties: [Option<MeilisearchNcDifficulty>; 5],
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct MeilisearchNcDifficulty {
	pub arcade: Option<MeilisearchNcChart>,
	pub console: Option<MeilisearchNcChart>,
	pub mixed: Option<MeilisearchNcChart>,
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct MeilisearchNcChart {
	// If is none its inhereted from the songs existing pv_db
	pub level: Option<pv_db::Level>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct MeilisearchDbEntry {
	pub uid: u64,
	pub post_id: i32,
	pub id: u32,
	pub name: String,
}

#[derive(Serialize, Deserialize)]
struct Config {
	include: Option<Vec<String>>,
}

pub const ROM_DIRS: [&'static str; 31] = [
	".",
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
	if post.post_type == PostType::Cover {
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
					let folder = format!("{}/{include}/{rom}/rom", file.parent()?.to_str()?);
					let path = Path::new(&folder);
					if !path.exists() {
						continue;
					}

					let nc_db = format!("{folder}/nc_db.toml");
					let path = Path::new(&nc_db);
					if path.exists() {
						if let Ok(data) = tokio::fs::read_to_string(&path).await {
							parse_nc_db(&data, post_id, state.clone()).await;
						}
					}

					for prefix in &DB_PREFIXES {
						let pv_db = format!("{folder}/{prefix}pv_db.txt");
						let path = Path::new(&pv_db);
						if path.exists() {
							if let Ok(data) = tokio::fs::read_to_string(&path).await {
								parse_pv_db(&data, post_id, state.clone()).await;
							}
						}

						let spr_db = format!("/{folder}/2d/{prefix}spr_db.bin");
						let path = Path::new(&spr_db);
						if path.exists() {
							parse_spr_db(spr_db, post_id, &state).await;
						}

						let aet_db = format!("/{folder}/2d/{prefix}aet_db.bin");
						let path = Path::new(&aet_db);
						if path.exists() {
							parse_aet_db(aet_db, post_id, &state).await;
						}

						let obj_db = format!("/{folder}/objset/{prefix}obj_db.bin");
						let path = Path::new(&obj_db);
						if path.exists() {
							parse_obj_db(obj_db, post_id, &state).await;
						}

						let tex_db = format!("/{folder}/objset/{prefix}tex_db.bin");
						let path = Path::new(&tex_db);
						if path.exists() {
							parse_tex_db(tex_db, post_id, &state).await;
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

pub async fn parse_spr_db<P: AsRef<Path>>(path: P, post_id: i32, state: &AppState) -> Option<()> {
	let spr_db = diva_db::SprDb::from_file(path).ok()?;

	let mut sets = Vec::new();
	let mut sprites = Vec::new();
	for (id, set) in spr_db.sets {
		sets.push(MeilisearchDbEntry {
			uid: (post_id as u64) << 32 | (id as u64),
			post_id,
			id,
			name: set.name,
		});

		for (id, sprite) in set.sprites {
			sprites.push(MeilisearchDbEntry {
				uid: (post_id as u64) << 32 | (id as u64),
				post_id,
				id,
				name: sprite.name,
			});
		}

		for (id, texture) in set.textures {
			sprites.push(MeilisearchDbEntry {
				uid: (post_id as u64) << 32 | (id as u64),
				post_id,
				id,
				name: texture.name,
			});
		}
	}

	let base =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("sprite_sets"))
			.with_filter("post_id=-1")
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchDbEntry>()
			.await
			.ok()?;

	let sets = sets
		.into_iter()
		.filter(|entry| {
			!base
				.results
				.iter()
				.any(|base| base.id == entry.id && base.name == entry.name)
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("sprite_sets")
		.add_or_update(&sets, Some("uid"))
		.await
		.ok()?;

	let base = meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("sprites"))
		.with_filter("post_id=-1")
		.with_limit(u32::MAX as usize)
		.execute::<MeilisearchDbEntry>()
		.await
		.ok()?;

	let sprites = sprites
		.into_iter()
		.filter(|entry| {
			!base
				.results
				.iter()
				.any(|base| base.id == entry.id && base.name == entry.name)
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("sprites")
		.add_or_update(&sprites, Some("uid"))
		.await
		.ok()?;

	Some(())
}

async fn parse_aet_db<P: AsRef<Path>>(path: P, post_id: i32, state: &AppState) -> Option<()> {
	let aet_db = diva_db::AetDb::from_file(path).ok()?;

	let mut set_entries = Vec::new();
	let mut scene_entries = Vec::new();
	for (id, set) in aet_db.sets {
		set_entries.push(MeilisearchDbEntry {
			uid: (post_id as u64) << 32 | (id as u64),
			post_id,
			id,
			name: set.name,
		});

		for (id, scene) in set.scenes {
			scene_entries.push(MeilisearchDbEntry {
				uid: (post_id as u64) << 32 | (id as u64),
				post_id,
				id,
				name: scene.name,
			});
		}
	}

	let base_sets =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aet_sets"))
			.with_filter("post_id=-1")
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchDbEntry>()
			.await
			.ok()?;

	let set_entries = set_entries
		.into_iter()
		.filter(|entry| {
			!base_sets
				.results
				.iter()
				.any(|base| base.id == entry.id && base.name == entry.name)
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("aet_sets")
		.add_or_update(&set_entries, Some("uid"))
		.await
		.ok()?;

	let base_scenes =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aet_scenes"))
			.with_filter("post_id=-1")
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchDbEntry>()
			.await
			.ok()?;

	let scene_entries = scene_entries
		.into_iter()
		.filter(|entry| {
			!base_scenes
				.results
				.iter()
				.any(|base| base.id == entry.id && base.name == entry.name)
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("aet_scenes")
		.add_or_update(&scene_entries, Some("uid"))
		.await
		.ok()?;

	Some(())
}

async fn parse_obj_db<P: AsRef<Path>>(path: P, post_id: i32, state: &AppState) -> Option<()> {
	let obj_db = diva_db::ObjDb::from_file(path).ok()?;

	let mut entries = Vec::new();
	for (id, set) in obj_db.sets {
		entries.push(MeilisearchDbEntry {
			uid: (post_id as u64) << 32 | (id as u64),
			post_id,
			id,
			name: set.name,
		});
	}

	let base = meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("objsets"))
		.with_filter("post_id=-1")
		.with_limit(u32::MAX as usize)
		.execute::<MeilisearchDbEntry>()
		.await
		.ok()?;

	let entries = entries
		.into_iter()
		.filter(|entry| {
			!base
				.results
				.iter()
				.any(|base| base.id == entry.id && base.name == entry.name)
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("objsets")
		.add_or_update(&entries, Some("uid"))
		.await
		.ok()?;

	Some(())
}

async fn parse_tex_db<P: AsRef<Path>>(path: P, post_id: i32, state: &AppState) -> Option<()> {
	let tex_db = diva_db::TexDb::from_file(path).ok()?;

	let mut entries = Vec::new();
	for (id, name) in tex_db.textures {
		entries.push(MeilisearchDbEntry {
			uid: (post_id as u64) << 32 | (id as u64),
			post_id,
			id,
			name,
		});
	}

	let base =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("textures"))
			.with_filter("post_id=-1")
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchDbEntry>()
			.await
			.ok()?;

	let entries = entries
		.into_iter()
		.filter(|entry| {
			!base
				.results
				.iter()
				.any(|base| base.id == entry.id && base.name == entry.name)
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("textures")
		.add_or_update(&entries, Some("uid"))
		.await
		.ok()?;

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
		module_db::ModuleDb::from_files(module_tbl, customize_item_tbl, chritm_prop, str_array)?;

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

	let base = meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("modules"))
		.with_filter("post_id=-1")
		.with_limit(u32::MAX as usize)
		.execute::<MeilisearchModule>()
		.await
		.ok()?;

	let mut modules = modules
		.into_iter()
		.filter(|module| {
			!base.results.iter().any(|base| {
				base.module_id == module.module_id && base.module.name_jp == module.module.name_jp
			})
		})
		.collect::<Vec<_>>();

	for module in &mut modules {
		for item in &mut module.module.cos.items {
			if item.objset.is_empty() {
				for base_module in &base.results {
					if base_module.module.chara != module.module.chara {
						continue;
					}
					for base_item in &base_module.module.cos.items {
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

	let base =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("cstm_items"))
			.with_filter("post_id=-1")
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchCstmItem>()
			.await
			.ok()?;

	let cstm_items = cstm_items
		.into_iter()
		.filter(|cstm_item| {
			!base.results.iter().any(|base| {
				base.customize_item_id == cstm_item.customize_item_id
					&& base.customize_item.name_jp == cstm_item.customize_item.name_jp
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

	let base = meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("pvs"))
		.with_filter("post=-1")
		.with_limit(u32::MAX as usize)
		.execute::<MeilisearchPv>()
		.await
		.unwrap();

	let pvs = documents
		.into_iter()
		.filter(|pv| {
			!base.results.iter().any(|base| {
				base.pv_id == pv.pv_id
					&& base.song_name == pv.song_name
					&& base.song_name_en == pv.song_name_en
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

#[derive(Serialize, Deserialize)]
struct NcDb {
	songs: Vec<NcDbSong>,
}

#[derive(Serialize, Deserialize)]
struct NcDbSong {
	id: i32,
	easy: Option<Vec<NcDbDifficulty>>,
	normal: Option<Vec<NcDbDifficulty>>,
	hard: Option<Vec<NcDbDifficulty>>,
	extreme: Option<Vec<NcDbDifficulty>>,
	ex_extreme: Option<Vec<NcDbDifficulty>>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum NcDbStyle {
	Arcade,
	Console,
	Mixed,
}

#[derive(Serialize, Deserialize)]
struct NcDbDifficulty {
	style: NcDbStyle,
	level: Option<pv_db::Level>,
}

pub async fn parse_nc_db(data: &str, post_id: i32, state: AppState) -> Option<()> {
	let nc_db: NcDb = toml::from_str(data).ok()?;

	let songs = nc_db
		.songs
		.iter()
		.map(|song| MeilisearchNcSong {
			uid: (post_id as u64) << 32 | (song.id as u64),
			post_id,
			pv_id: song.id,
			difficulties: [
				song.easy
					.as_ref()
					.map(|difficulty| MeilisearchNcDifficulty {
						arcade: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Arcade)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						console: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Console)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						mixed: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Mixed)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
					}),
				song.normal
					.as_ref()
					.map(|difficulty| MeilisearchNcDifficulty {
						arcade: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Arcade)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						console: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Console)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						mixed: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Mixed)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
					}),
				song.hard
					.as_ref()
					.map(|difficulty| MeilisearchNcDifficulty {
						arcade: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Arcade)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						console: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Console)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						mixed: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Mixed)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
					}),
				song.extreme
					.as_ref()
					.map(|difficulty| MeilisearchNcDifficulty {
						arcade: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Arcade)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						console: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Console)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						mixed: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Mixed)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
					}),
				song.ex_extreme
					.as_ref()
					.map(|difficulty| MeilisearchNcDifficulty {
						arcade: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Arcade)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						console: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Console)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
						mixed: difficulty
							.iter()
							.find(|chart| chart.style == NcDbStyle::Mixed)
							.map(|chart| MeilisearchNcChart {
								level: chart.level.clone(),
							}),
					}),
			],
		})
		.collect::<Vec<_>>();

	state
		.meilisearch
		.index("nc_songs")
		.add_or_update(&songs, Some("uid"))
		.await
		.unwrap();

	Some(())
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
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

	pub fn owns_nc(&self, nc_songs: &[NcSong]) -> bool {
		let Some(post) = self.post else { return false };

		nc_songs.iter().any(|nc_song| nc_song.post == post)
	}
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct Module {
	pub uid: String,
	pub post: Option<i32>,
	pub id: i32,
	pub module: module_db::Module,
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct CstmItem {
	pub uid: String,
	pub post: Option<i32>,
	pub id: i32,
	pub cstm_item: module_db::CustomizeItem,
}

#[derive(Serialize, Deserialize, Default, ToSchema)]
pub struct NcSong {
	pub uid: String,
	pub post: i32,
	pub pv_id: i32,
	pub difficulties: [Option<MeilisearchNcDifficulty>; 5],
}

impl NcSong {
	pub fn has_arcade(&self, pv: &Pv) -> bool {
		for i in 0..5 {
			if self.difficulties[i].as_ref().map_or(true, |diff| {
				(diff.console.is_none() && diff.mixed.is_none()) || diff.arcade.is_some()
			}) && pv.levels[i].is_some()
			{
				return true;
			}
		}

		false
	}

	pub fn has_console(&self) -> bool {
		self.difficulties
			.iter()
			.filter_map(|diff| diff.clone())
			.any(|diff| diff.console.is_some())
	}

	pub fn has_mixed(&self) -> bool {
		self.difficulties
			.iter()
			.filter_map(|diff| diff.clone())
			.any(|diff| diff.mixed.is_some())
	}

	pub fn has_arcade_search(&self, search: &NcSongSearch) -> bool {
		let Some(pvs) = search.pvs.get(&self.pv_id) else {
			if self
				.difficulties
				.iter()
				.filter_map(|diff| diff.clone())
				.any(|diff| diff.arcade.is_some())
			{
				return true;
			}
			return false;
		};
		if pvs.len() == 0 {
			if self
				.difficulties
				.iter()
				.filter_map(|diff| diff.clone())
				.any(|diff| diff.arcade.is_some())
			{
				return true;
			}
			return false;
		}

		for i in 0..5 {
			if self.difficulties[i].as_ref().map_or(true, |diff| {
				(diff.console.is_none() && diff.mixed.is_none()) || diff.arcade.is_some()
			}) {
				if pvs.iter().any(|pv| pv.levels[i].is_some()) {
					return true;
				}
			}
		}

		false
	}

	pub fn get_arcade_level(&self, pv: &Pv, difficulty: usize) -> Option<pv_db::Level> {
		if pv.levels[difficulty].is_none()
			|| self.difficulties[difficulty]
				.as_ref()
				.map_or(false, |diff| {
					diff.arcade.is_none() && (diff.console.is_some() || diff.mixed.is_some())
				}) {
			return None;
		}

		if let Some(diff) = &self.difficulties[difficulty] {
			if let Some(arcade) = &diff.arcade {
				if arcade.level.is_some() {
					return arcade.level.clone();
				}
			}
		}

		pv.levels[difficulty].clone()
	}

	pub fn get_console_level(&self, pv: &Pv, difficulty: usize) -> Option<pv_db::Level> {
		if pv.levels[difficulty].is_none()
			|| self.difficulties[difficulty]
				.as_ref()
				.map_or(true, |diff| diff.console.is_none())
		{
			return None;
		}

		if let Some(diff) = &self.difficulties[difficulty] {
			if let Some(console) = &diff.console {
				if console.level.is_some() {
					return console.level.clone();
				}
			}
		}

		pv.levels[difficulty].clone()
	}

	pub fn get_mixed_level(&self, pv: &Pv, difficulty: usize) -> Option<pv_db::Level> {
		if pv.levels[difficulty].is_none()
			|| self.difficulties[difficulty]
				.as_ref()
				.map_or(true, |diff| diff.mixed.is_none())
		{
			return None;
		}

		if let Some(diff) = &self.difficulties[difficulty] {
			if let Some(mixed) = &diff.mixed {
				if mixed.level.is_some() {
					return mixed.level.clone();
				}
			}
		}

		pv.levels[difficulty].clone()
	}

	pub fn get_console_level_search(
		&self,
		search: &NcSongSearch,
		difficulty: usize,
	) -> Option<Option<pv_db::Level>> {
		let Some(pv_level) = search.get_pv_level(self.pv_id, difficulty) else {
			if search
				.pvs
				.get(&self.pv_id)
				.map_or(true, |pvs| pvs.len() == 0)
			{
				if let Some(diff) = &self.difficulties[difficulty] {
					if let Some(console) = &diff.console {
						if console.level.is_some() {
							return Some(console.level.clone());
						} else {
							return Some(None);
						}
					}
				}
			}
			return None;
		};

		if self.difficulties[difficulty]
			.as_ref()
			.map_or(true, |diff| diff.console.is_none())
		{
			return None;
		}

		if let Some(diff) = &self.difficulties[difficulty] {
			if let Some(console) = &diff.console {
				if console.level.is_some() {
					return Some(console.level.clone());
				}
			}
		}

		Some(Some(pv_level))
	}

	pub fn get_mixed_level_search(
		&self,
		search: &NcSongSearch,
		difficulty: usize,
	) -> Option<Option<pv_db::Level>> {
		let Some(pv_level) = search.get_pv_level(self.pv_id, difficulty) else {
			if search
				.pvs
				.get(&self.pv_id)
				.map_or(true, |pvs| pvs.len() == 0)
			{
				if let Some(diff) = &self.difficulties[difficulty] {
					if let Some(mixed) = &diff.mixed {
						if mixed.level.is_some() {
							return Some(mixed.level.clone());
						} else {
							return Some(None);
						}
					}
				}
			}
			return None;
		};

		if self.difficulties[difficulty]
			.as_ref()
			.map_or(true, |diff| diff.mixed.is_none())
		{
			return None;
		}

		if let Some(diff) = &self.difficulties[difficulty] {
			if let Some(mixed) = &diff.mixed {
				if mixed.level.is_some() {
					return Some(mixed.level.clone());
				}
			}
		}

		Some(Some(pv_level))
	}
}

#[derive(Serialize, Deserialize, Default, ToSchema)]
pub struct PvSearch {
	pub pvs: Vec<Pv>,
	pub nc_songs: BTreeMap<i32, Vec<NcSong>>,
	pub posts: BTreeMap<i32, Post>,
}

impl PvSearch {
	pub fn find_pv(&self, id: i32) -> Option<&Pv> {
		self.pvs.iter().find(|pv| pv.id == id)
	}
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/pvs",
	params(
		SearchParams
	),
	responses(
		(status = 200, body = PvSearch, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn search_pvs(
	Query(query): Query<SearchParams>,
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

	let mut pvs = search
		.execute::<MeilisearchPv>()
		.await
		.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

	let mut hits = pvs.hits.into_iter().map(|p| p.result).collect::<Vec<_>>();

	while pvs.estimated_total_hits.unwrap_or(0) > hits.len()
		&& query.limit.unwrap_or(u32::MAX as usize) > hits.len()
	{
		search.offset = Some(hits.len() + query.offset.unwrap_or(0));
		pvs = search
			.execute::<MeilisearchPv>()
			.await
			.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

		let mut results = pvs.hits.into_iter().map(|p| p.result).collect::<Vec<_>>();
		hits.append(&mut results);
	}

	let mut vec = Vec::with_capacity(hits.len());
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();
	for pv in hits {
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

	let mut nc_songs: BTreeMap<i32, Vec<NcSong>> = BTreeMap::new();

	if vec.len() > 0 {
		let filter = vec
			.iter()
			.map(|pv| format!("pv_id={}", pv.id))
			.intersperse(String::from(" OR "))
			.collect::<String>();

		let search =
			meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("nc_songs"))
				.with_limit(u32::MAX as usize)
				.with_filter(&filter)
				.execute::<MeilisearchNcSong>()
				.await;

		if let Ok(result) = search {
			for nc_song in result.results {
				if !posts.contains_key(&nc_song.post_id) {
					if let Some(mut post) = Post::get_full(nc_song.post_id, &state.db).await {
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
					}
				}

				if let Some(nc_vec) = nc_songs.get_mut(&nc_song.pv_id) {
					nc_vec.push(NcSong {
						uid: BASE64_STANDARD.encode(nc_song.uid.to_ne_bytes()),
						post: nc_song.post_id,
						pv_id: nc_song.pv_id,
						difficulties: nc_song.difficulties.clone(),
					});
				} else {
					nc_songs.insert(
						nc_song.pv_id,
						vec![NcSong {
							uid: BASE64_STANDARD.encode(nc_song.uid.to_ne_bytes()),
							post: nc_song.post_id,
							pv_id: nc_song.pv_id,
							difficulties: nc_song.difficulties.clone(),
						}],
					);
				}
			}
		};
	}

	Ok(Json(PvSearch {
		pvs: vec,
		nc_songs,
		posts,
	}))
}

#[derive(Serialize, Deserialize, Default, ToSchema)]
pub struct ModuleSearch {
	pub modules: Vec<Module>,
	pub posts: BTreeMap<i32, Post>,
}

impl ModuleSearch {
	pub fn find_module(&self, id: i32) -> Option<&Module> {
		self.modules.iter().find(|module| module.id == id)
	}

	pub fn find_cos(&self, id: i32) -> Option<&Module> {
		self.modules
			.iter()
			.find(|module| module.module.cos.id == id)
	}
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/modules",
	params(
		SearchParams
	),
	responses(
		(status = 200, body = ModuleSearch, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn search_modules(
	Query(query): Query<SearchParams>,
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

	let mut modules = search
		.execute::<MeilisearchModule>()
		.await
		.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

	let mut hits = modules
		.hits
		.into_iter()
		.map(|module| module.result)
		.collect::<Vec<_>>();

	while modules.estimated_total_hits.unwrap_or(0) > hits.len()
		&& query.limit.unwrap_or(u32::MAX as usize) > hits.len()
	{
		search.offset = Some(hits.len() + query.offset.unwrap_or(0));
		modules = search
			.execute::<MeilisearchModule>()
			.await
			.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

		let mut results = modules
			.hits
			.into_iter()
			.map(|module| module.result)
			.collect::<Vec<_>>();
		hits.append(&mut results);
	}

	let mut vec = Vec::with_capacity(hits.len());
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();
	for module in hits {
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

#[derive(Serialize, Deserialize, Default, ToSchema)]
pub struct CstmItemSearch {
	pub cstm_items: Vec<CstmItem>,
	pub bound_modules: BTreeMap<i32, Module>,
	pub posts: BTreeMap<i32, Post>,
}

impl CstmItemSearch {
	pub fn find_cstm_item(&self, id: i32) -> Option<&CstmItem> {
		self.cstm_items.iter().find(|cstm_item| cstm_item.id == id)
	}
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/cstm_items",
	params(
		SearchParams
	),
	responses(
		(status = 200, body = CstmItemSearch, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn search_cstm_items(
	Query(query): Query<SearchParams>,
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

	let mut cstm_items = search
		.execute::<MeilisearchCstmItem>()
		.await
		.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

	let mut hits = cstm_items
		.hits
		.into_iter()
		.map(|cstm_item| cstm_item.result)
		.collect::<Vec<_>>();

	while cstm_items.estimated_total_hits.unwrap_or(0) > hits.len()
		&& query.limit.unwrap_or(u32::MAX as usize) > hits.len()
	{
		search.offset = Some(hits.len() + query.offset.unwrap_or(0));
		cstm_items = search
			.execute::<MeilisearchCstmItem>()
			.await
			.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

		let mut results = cstm_items
			.hits
			.into_iter()
			.map(|cstm_item| cstm_item.result)
			.collect::<Vec<_>>();
		hits.append(&mut results);
	}

	let mut vec = Vec::with_capacity(hits.len());
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();
	let mut pending_bound_modules: BTreeSet<(i32, Option<i32>)> = BTreeSet::new();

	for cstm_item in hits {
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
			Query(crate::api::ids::SearchParams {
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

#[derive(Serialize, Deserialize, Default, ToSchema)]
pub struct NcSongSearch {
	pub nc_songs: Vec<NcSong>,
	pub pvs: BTreeMap<i32, Vec<Pv>>,
	pub posts: BTreeMap<i32, Post>,
}

impl NcSongSearch {
	pub fn contains_other_posts_songs(&self, post: &Post) -> bool {
		self.pvs
			.iter()
			.any(|(_, pvs)| pvs.iter().any(|pv| pv.post != Some(post.id)))
			|| self.pvs.is_empty()
	}

	pub fn get_pv_level(&self, pv_id: i32, difficulty_index: usize) -> Option<pv_db::Level> {
		let Some(pvs) = self.pvs.get(&pv_id) else {
			return None;
		};
		if pvs.len() == 0 {
			return None;
		}
		if !pvs.iter().all(|pv| pv.id == pvs[0].id) {
			return None;
		}

		if pvs
			.iter()
			.all(|pv| pv.levels[difficulty_index] == pvs[0].levels[difficulty_index])
		{
			pvs[0].levels[difficulty_index].clone()
		} else {
			None
		}
	}
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/nc_songs",
	params(
		SearchParams
	),
	responses(
		(status = 200, body = NcSongSearch, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn search_nc_songs(
	Query(query): Query<SearchParams>,
	State(state): State<AppState>,
) -> Result<Json<NcSongSearch>, (StatusCode, String)> {
	let index = state.meilisearch.index("nc_songs");
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

	let mut nc_songs = search
		.execute::<MeilisearchNcSong>()
		.await
		.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

	let mut hits = nc_songs
		.hits
		.into_iter()
		.map(|p| p.result)
		.collect::<Vec<_>>();

	while nc_songs.estimated_total_hits.unwrap_or(0) > hits.len()
		&& query.limit.unwrap_or(u32::MAX as usize) > hits.len()
	{
		search.offset = Some(hits.len() + query.offset.unwrap_or(0));
		nc_songs = search
			.execute::<MeilisearchNcSong>()
			.await
			.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

		let mut results = nc_songs
			.hits
			.into_iter()
			.map(|p| p.result)
			.collect::<Vec<_>>();
		hits.append(&mut results);
	}

	let mut vec = Vec::with_capacity(hits.len());
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();

	for nc_song in hits {
		if !posts.contains_key(&nc_song.post_id) {
			if let Some(mut post) = Post::get_full(nc_song.post_id, &state.db).await {
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
			}
		}

		vec.push(NcSong {
			uid: BASE64_STANDARD.encode(nc_song.uid.to_ne_bytes()),
			post: nc_song.post_id,
			pv_id: nc_song.pv_id,
			difficulties: nc_song.difficulties.clone(),
		});
	}

	let mut pvs: BTreeMap<i32, Vec<Pv>> = BTreeMap::new();
	if vec.len() > 0 {
		let filter = vec
			.iter()
			.map(|pv| format!("pv_id={}", pv.pv_id))
			.intersperse(String::from(" OR "))
			.collect::<String>();

		let search =
			meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("pvs"))
				.with_limit(u32::MAX as usize)
				.with_filter(&filter)
				.execute::<MeilisearchPv>()
				.await;

		if let Ok(result) = search {
			for pv in result.results {
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

				if let Some(pv_vec) = pvs.get_mut(&pv.pv_id) {
					pv_vec.push(Pv {
						uid: BASE64_STANDARD.encode(pv.uid.to_ne_bytes()),
						id: pv.pv_id,
						name: pv.song_name,
						name_en: pv.song_name_en,
						song_info: pv.song_info,
						song_info_en: pv.song_info_en,
						levels: pv.levels,
						post,
					});
				} else {
					pvs.insert(
						pv.pv_id,
						vec![Pv {
							uid: BASE64_STANDARD.encode(pv.uid.to_ne_bytes()),
							id: pv.pv_id,
							name: pv.song_name,
							name_en: pv.song_name_en,
							song_info: pv.song_info,
							song_info_en: pv.song_info_en,
							levels: pv.levels,
							post,
						}],
					);
				};
			}
		};
	}

	Ok(Json(NcSongSearch {
		nc_songs: vec,
		pvs,
		posts,
	}))
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, ToSchema)]
#[repr(i32)]
pub enum ReservationType {
	Song = 0,
	Module = 1,
	CstmItem = 2,
	CosMiku = 10,
	CosRin = 11,
	CosLen = 12,
	CosLuka = 13,
	CosNeru = 14,
	CosHaku = 15,
	CosKaito = 16,
	CosMeiko = 17,
	CosSakine = 18,
	CosTeto = 19,
}

impl From<i32> for ReservationType {
	fn from(value: i32) -> Self {
		match value {
			1 => Self::Module,
			2 => Self::CstmItem,
			10 => Self::CosMiku,
			11 => Self::CosRin,
			12 => Self::CosLen,
			13 => Self::CosLuka,
			14 => Self::CosNeru,
			15 => Self::CosHaku,
			16 => Self::CosKaito,
			17 => Self::CosMeiko,
			18 => Self::CosSakine,
			19 => Self::CosTeto,
			_ => Self::Song,
		}
	}
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ReservationRange {
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
	InvalidLength(usize),        // The length is too high, returns the users max remaining length
	InvalidAlignment(i32),       // Start was improperly aligned
}

#[derive(Serialize, Deserialize)]
pub struct ReserveRangeArgs {
	pub reservation_type: ReservationType,
	pub start: i32,
	pub length: i32,
}

pub async fn create_reservation(
	user: User,
	State(state): State<AppState>,
	Json(query): Json<ReserveRangeArgs>,
) -> Json<ReserveRangeResult> {
	if query.reservation_type != ReservationType::Song
		|| query.start < 1
		|| query.length < 1
		|| query.start.checked_add(query.length).is_none()
	{
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
				query.start,
				query.length,
				time
			)
			.execute(&state.db)
			.await;
		}
		ReserveRangeResult::PartialValidRange(ref old_ids) => {
			let old_ids = old_ids.iter().cloned().collect::<BTreeSet<_>>();
			let new_ids = (query.start..(query.start + query.length)).collect::<BTreeSet<_>>();

			let time = time::OffsetDateTime::now_utc();

			let mut ranges: Vec<ReservationRange> = Vec::new();
			for id in new_ids.difference(&old_ids) {
				if let Some(last) = ranges.last_mut() {
					if last.range_start + last.length == *id {
						last.length += 1;
					} else {
						ranges.push(ReservationRange {
							user: user.clone(),
							reservation_type: query.reservation_type,
							range_start: *id,
							length: 1,
							time,
						});
					}
				} else {
					ranges.push(ReservationRange {
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

	for id in query.start..(query.start + query.length) {
		_ = sqlx::query!(
			"DELETE FROM reservation_labels WHERE reservation_type = $1 AND id = $2 AND user_id = $3",
			query.reservation_type as i32,
			id,
			user.id
		)
		.execute(&state.db)
		.await;
	}

	let reservered_ids = sqlx::query!(
		r#"
		SELECT * FROM reservations r
		WHERE r.reservation_type = $1
		AND r.user_id = $2
		AND (r.range_start = $3 OR r.range_start + r.length > $3) AND r.range_start < $4
		"#,
		query.reservation_type as i32,
		user.id,
		query.start,
		(query.start + query.length)
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

	let ids = (query.start..(query.start + query.length)).collect::<BTreeSet<_>>();

	if ids.len() > reservered_ids.len() {
		return;
	}

	let mut ranges: Vec<ReservationRange> = Vec::new();
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
				ranges.push(ReservationRange {
					user: user.clone(),
					reservation_type: query.reservation_type,
					range_start: *id,
					length: 1,
					time: reservered_ids[id],
				});
			}
		} else {
			ranges.push(ReservationRange {
				user: user.clone(),
				reservation_type: query.reservation_type,
				range_start: *id,
				length: 1,
				time: reservered_ids[id],
			});
		}
	}

	if let Ok(mut transaction) = state.db.begin().await {
		_ = sqlx::query!(
			r#"
			DELETE FROM reservations r
			WHERE r.reservation_type = $1
			AND r.user_id = $2
			AND (r.range_start = $3 OR r.range_start + r.length > $3) AND r.range_start < $4
			"#,
			query.reservation_type as i32,
			user.id,
			query.start,
			(query.start + query.length)
		)
		.execute(&mut *transaction)
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
			.execute(&mut *transaction)
			.await;
		}

		_ = transaction.commit().await;
	}
}

#[derive(Serialize, Deserialize)]
pub struct LabelReservationArgs {
	pub reservation_type: ReservationType,
	pub label: String,
}

pub async fn label_reservation(
	axum::extract::Path(id): axum::extract::Path<i32>,
	user: User,
	State(state): State<AppState>,
	Json(query): Json<LabelReservationArgs>,
) -> StatusCode {
	if id < 1 {
		return StatusCode::BAD_REQUEST;
	}

	let has_reservation = sqlx::query!(
		r#"
		SELECT COUNT(*) FROM reservations r
		WHERE r.reservation_type = $1
		AND r.user_id = $2
		AND (r.range_start = $3 OR r.range_start + r.length > $3) AND r.range_start < $3 + 1
		"#,
		query.reservation_type as i32,
		user.id,
		id,
	)
	.fetch_one(&state.db)
	.await
	.map_or(false, |count| count.count.unwrap_or(0) > 0);

	if !has_reservation {
		return StatusCode::BAD_REQUEST;
	}

	if sqlx::query!(
		r#"
		UPDATE reservation_labels SET label=$1 WHERE user_id=$2 AND reservation_type=$3 AND id=$4
		"#,
		query.label,
		user.id,
		query.reservation_type as i32,
		id
	)
	.execute(&state.db)
	.await
	.unwrap_or_default()
	.rows_affected()
		== 0
	{
		_ = sqlx::query!(
			"INSERT INTO reservation_labels VALUES ($1, $2, $3, $4)",
			user.id,
			query.reservation_type as i32,
			id,
			query.label
		)
		.execute(&state.db)
		.await;
	}

	StatusCode::OK
}

pub async fn web_check_reserve_range(
	Query(query): Query<ReserveRangeArgs>,
	user: User,
	State(state): State<AppState>,
) -> Json<ReserveRangeResult> {
	if query.start < 0 || query.length < 0 {
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
- Max number of reserved ids is 50 + half of how many items the user has already uploaded rounded up to the nearest multiple of 10, e.g. if a user has uploaded a song pack with 30 songs they can reserve 70 song ids and 50 module/cstm_item ids
*/

pub async fn check_reserve_range(
	reservation_type: ReservationType,
	start: i32,
	length: i32,
	user: &User,
	state: &AppState,
) -> ReserveRangeResult {
	if start < 1 || length < 1 || start.checked_add(length).is_none() {
		return ReserveRangeResult::InvalidRange;
	}

	let max = get_user_max_reservations(reservation_type, &user, &state).await;
	if max < length as usize {
		return ReserveRangeResult::InvalidLength(max);
	}

	let alignment = 10_i32
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

			let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
				.with_limit(u32::MAX as usize)
				.with_filter(&filter)
				.execute::<MeilisearchPv>()
				.await;

			search.map_or(BTreeMap::new(), |search| {
				search
					.results
					.into_iter()
					.map(|pv| (pv.pv_id, pv.post))
					.collect::<BTreeMap<_, _>>()
			})
		}
		ReservationType::Module => {
			let index = state.meilisearch.index("modules");

			let filter = (start..(start + length))
				.map(|id| format!("module_id={id}"))
				.intersperse(String::from(" OR "))
				.collect::<String>();

			let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
				.with_limit(u32::MAX as usize)
				.with_filter(&filter)
				.execute::<MeilisearchModule>()
				.await;

			search.map_or(BTreeMap::new(), |search| {
				search
					.results
					.into_iter()
					.map(|module| (module.module_id, module.post_id))
					.collect::<BTreeMap<_, _>>()
			})
		}
		ReservationType::CstmItem => {
			let index = state.meilisearch.index("cstm_items");

			let filter = (start..(start + length))
				.map(|id| format!("customize_item_id={id}"))
				.intersperse(String::from(" OR "))
				.collect::<String>();

			let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
				.with_limit(u32::MAX as usize)
				.with_filter(&filter)
				.execute::<MeilisearchCstmItem>()
				.await;

			search.map_or(BTreeMap::new(), |search| {
				search
					.results
					.into_iter()
					.map(|cstm_item| (cstm_item.customize_item_id, cstm_item.post_id))
					.collect::<BTreeMap<_, _>>()
			})
		}
		ReservationType::CosMiku
		| ReservationType::CosRin
		| ReservationType::CosLen
		| ReservationType::CosLuka
		| ReservationType::CosNeru
		| ReservationType::CosHaku
		| ReservationType::CosKaito
		| ReservationType::CosMeiko
		| ReservationType::CosSakine
		| ReservationType::CosTeto => {
			let chara = module_db::Chara::try_from(reservation_type as i32 - 10).unwrap();
			let index = state.meilisearch.index("modules");

			let filter = (start..(start + length))
				.map(|id| format!("cos.id={id}"))
				.intersperse(String::from(" OR "))
				.collect::<String>();

			let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
				.with_limit(u32::MAX as usize)
				.with_filter(&format!(
					"({filter}) AND chara={}",
					serde_json::to_string(&chara).unwrap().trim_matches('\"'),
				))
				.execute::<MeilisearchModule>()
				.await;

			search.map_or(BTreeMap::new(), |search| {
				search
					.results
					.into_iter()
					.map(|module| (module.module.cos.id, module.post_id))
					.collect::<BTreeMap<_, _>>()
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
		"SELECT u.id, r.range_start, r.length FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE (r.range_start = $1 OR r.range_start + r.length > $1) AND r.range_start < $2 AND r.reservation_type = $3",
		start,
		(start + length),
		reservation_type as i32,
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default();

	for conflict in conflicts {
		if conflict.id == user.id {
			for conflict in conflict.range_start..(conflict.range_start + conflict.length) {
				if conflict >= start && conflict < (start + length) {
					partial_range.push(conflict);
				}
			}
		} else {
			return ReserveRangeResult::InvalidRange;
		}
	}

	if partial_range == ((start)..(start + length)).collect::<Vec<_>>() {
		return ReserveRangeResult::InvalidRange;
	}

	if partial_range.len() > 0 {
		return ReserveRangeResult::PartialValidRange(partial_range);
	}

	ReserveRangeResult::ValidRange
}

pub async fn get_user_uploads(
	reservation_type: ReservationType,
	user: &User,
	state: &AppState,
) -> BTreeSet<i32> {
	let user_posts = sqlx::query!(
		"SELECT post_id FROM post_authors WHERE user_id = $1",
		user.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default();

	if user_posts.len() > 0 {
		match reservation_type {
			ReservationType::Song => {
				let index = state.meilisearch.index("pvs");

				let filter = user_posts
					.iter()
					.map(|post| format!("post={}", post.post_id))
					.intersperse(String::from(" OR "))
					.collect::<String>();

				let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
					.with_limit(u32::MAX as usize)
					.with_filter(&filter)
					.execute::<MeilisearchPv>()
					.await;

				search.map_or(BTreeSet::new(), |search| {
					search
						.results
						.into_iter()
						.map(|pv| pv.pv_id)
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

				let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
					.with_limit(u32::MAX as usize)
					.with_filter(&filter)
					.execute::<MeilisearchModule>()
					.await;

				search.map_or(BTreeSet::new(), |search| {
					search
						.results
						.into_iter()
						.map(|module| module.module_id)
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

				let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
					.with_limit(u32::MAX as usize)
					.with_filter(&filter)
					.execute::<MeilisearchCstmItem>()
					.await;

				search.map_or(BTreeSet::new(), |search| {
					search
						.results
						.into_iter()
						.map(|cstm_item| cstm_item.customize_item_id)
						.collect::<BTreeSet<_>>()
				})
			}
			ReservationType::CosMiku
			| ReservationType::CosRin
			| ReservationType::CosLen
			| ReservationType::CosLuka
			| ReservationType::CosNeru
			| ReservationType::CosHaku
			| ReservationType::CosKaito
			| ReservationType::CosMeiko
			| ReservationType::CosSakine
			| ReservationType::CosTeto => {
				let chara = module_db::Chara::try_from(reservation_type as i32 - 10).unwrap();
				let index = state.meilisearch.index("modules");

				let filter = user_posts
					.iter()
					.map(|post| format!("post_id={}", post.post_id))
					.intersperse(String::from(" OR "))
					.collect::<String>();

				let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
					.with_limit(u32::MAX as usize)
					.with_filter(&format!(
						"({filter}) AND chara={}",
						serde_json::to_string(&chara).unwrap().trim_matches('\"'),
					))
					.execute::<MeilisearchModule>()
					.await;

				search.map_or(BTreeSet::new(), |search| {
					search
						.results
						.into_iter()
						.map(|module| module.module.cos.id)
						.collect::<BTreeSet<_>>()
				})
			}
		}
	} else {
		BTreeSet::new()
	}
}

pub async fn get_user_reservations(
	reservation_type: ReservationType,
	user: &User,
	state: &AppState,
) -> BTreeSet<i32> {
	let existing_reservations = sqlx::query!(
		"SELECT range_start, length FROM reservations WHERE reservation_type = $1 AND user_id = $2 AND time != '1970-01-01'",
		reservation_type as i32,
		user.id
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default();

	let uploads = get_user_uploads(reservation_type, user, state).await;

	existing_reservations
		.iter()
		.flat_map(|reservation| {
			(reservation.range_start)..(reservation.range_start + reservation.length)
		})
		.filter(|reservation| !uploads.contains(reservation))
		.collect()
}

pub async fn get_user_max_reservations(
	reservation_type: ReservationType,
	user: &User,
	state: &AppState,
) -> usize {
	let uploads = get_user_uploads(reservation_type, user, state).await;
	let reservations = get_user_reservations(reservation_type, user, state)
		.await
		.len();
	50 + (uploads.len() / 2).next_multiple_of(10) - reservations
}

pub async fn web_find_reserve_range(
	Query(query): Query<ReserveRangeArgs>,
	user: User,
	State(state): State<AppState>,
) -> Json<i32> {
	Json(find_reservable_range(query.reservation_type, query.length, &user, &state).await)
}

pub async fn find_reservable_range(
	reservation_type: ReservationType,
	length: i32,
	user: &User,
	state: &AppState,
) -> i32 {
	if length < 1 {
		return -1;
	}

	let max = get_user_max_reservations(reservation_type, &user, &state).await;
	if max < length as usize {
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
		(reservation.range_start)..(reservation.range_start + reservation.length)
	})
	.collect::<BTreeSet<_>>();

	let mut ids = match reservation_type {
		ReservationType::Song => {
			let index = state.meilisearch.index("pvs");

			let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
				.with_limit(u32::MAX as usize)
				.execute::<MeilisearchPv>()
				.await;

			search.map_or(BTreeSet::new(), |search| {
				search
					.results
					.into_iter()
					.map(|pv| pv.pv_id)
					.collect::<BTreeSet<_>>()
			})
		}
		ReservationType::Module => {
			let index = state.meilisearch.index("modules");

			let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
				.with_limit(u32::MAX as usize)
				.execute::<MeilisearchModule>()
				.await;

			search.map_or(BTreeSet::new(), |search| {
				search
					.results
					.into_iter()
					.map(|module| module.module_id)
					.collect::<BTreeSet<_>>()
			})
		}
		ReservationType::CstmItem => {
			let index = state.meilisearch.index("cstm_items");

			let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
				.with_limit(u32::MAX as usize)
				.execute::<MeilisearchCstmItem>()
				.await;

			search.map_or(BTreeSet::new(), |search| {
				search
					.results
					.into_iter()
					.map(|cstm_item| cstm_item.customize_item_id)
					.collect::<BTreeSet<_>>()
			})
		}
		ReservationType::CosMiku
		| ReservationType::CosRin
		| ReservationType::CosLen
		| ReservationType::CosLuka
		| ReservationType::CosNeru
		| ReservationType::CosHaku
		| ReservationType::CosKaito
		| ReservationType::CosMeiko
		| ReservationType::CosSakine
		| ReservationType::CosTeto => {
			let chara = module_db::Chara::try_from(reservation_type as i32 - 10).unwrap();
			let index = state.meilisearch.index("modules");

			let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
				.with_limit(u32::MAX as usize)
				.with_filter(&format!(
					"chara={}",
					serde_json::to_string(&chara).unwrap().trim_matches('\"'),
				))
				.execute::<MeilisearchModule>()
				.await;

			search.map_or(BTreeSet::new(), |search| {
				search
					.results
					.into_iter()
					.map(|module| module.module.cos.id)
					.collect::<BTreeSet<_>>()
			})
		}
	};

	ids.append(&mut reservations);

	let alignment = 10_u32
		.checked_pow(length.checked_ilog10().unwrap_or(0))
		.unwrap_or(1);

	for (id, next) in ids.iter().tuple_windows() {
		let res = i32::try_from(
			u32::try_from(id + 1)
				.unwrap_or(0)
				.next_multiple_of(alignment),
		)
		.unwrap_or(-1);
		if res + length <= *next {
			return res;
		}
	}

	i32::try_from(
		u32::try_from(ids.last().map_or(0, |id| id + 1))
			.unwrap_or(0)
			.next_multiple_of(alignment),
	)
	.unwrap_or(-1)
}

pub async fn optimise_reservations(reservation_type: ReservationType, state: &AppState) {
	let users = sqlx::query_as!(
		User,
		r#"
		SELECT DISTINCT u.id, u.name, u.avatar, u.display_name, u.public_likes, u.theme, u.show_explicit
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

					let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
						.with_limit(u32::MAX as usize)
						.with_filter(&filter)
						.execute::<MeilisearchPv>()
						.await;

					search.map_or(BTreeSet::new(), |search| {
						search
							.results
							.into_iter()
							.map(|pv| pv.pv_id)
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

					let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
						.with_limit(u32::MAX as usize)
						.with_filter(&filter)
						.execute::<MeilisearchModule>()
						.await;

					search.map_or(BTreeSet::new(), |search| {
						search
							.results
							.into_iter()
							.map(|module| module.module_id)
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

					let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
						.with_limit(u32::MAX as usize)
						.with_filter(&filter)
						.execute::<MeilisearchCstmItem>()
						.await;

					search.map_or(BTreeSet::new(), |search| {
						search
							.results
							.into_iter()
							.map(|cstm_item| cstm_item.customize_item_id)
							.collect::<BTreeSet<_>>()
					})
				}
				ReservationType::CosMiku
				| ReservationType::CosRin
				| ReservationType::CosLen
				| ReservationType::CosLuka
				| ReservationType::CosNeru
				| ReservationType::CosHaku
				| ReservationType::CosKaito
				| ReservationType::CosMeiko
				| ReservationType::CosSakine
				| ReservationType::CosTeto => {
					let chara = module_db::Chara::try_from(reservation_type as i32 - 10).unwrap();
					let index = state.meilisearch.index("modules");

					let filter = user_posts
						.iter()
						.map(|post| format!("post_id={}", post.post_id))
						.intersperse(String::from(" OR "))
						.collect::<String>();

					let search = meilisearch_sdk::documents::DocumentsQuery::new(&index)
						.with_limit(u32::MAX as usize)
						.with_filter(&format!(
							"({filter}) AND chara={}",
							serde_json::to_string(&chara).unwrap().trim_matches('\"'),
						))
						.execute::<MeilisearchModule>()
						.await;

					search.map_or(BTreeSet::new(), |search| {
						search
							.results
							.into_iter()
							.map(|module| module.module.cos.id)
							.collect::<BTreeSet<_>>()
					})
				}
			}
		} else {
			BTreeSet::new()
		};

		let mut ranges: Vec<ReservationRange> = Vec::new();
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
					ranges.push(ReservationRange {
						user: user.clone(),
						reservation_type,
						range_start: *id,
						length: 1,
						time: reservered_ids[id],
					});
				}
			} else {
				ranges.push(ReservationRange {
					user: user.clone(),
					reservation_type,
					range_start: *id,
					length: 1,
					time: reservered_ids[id],
				});
			}
		}

		if let Ok(mut transaction) = state.db.begin().await {
			_ = sqlx::query!(
				r#"
				DELETE FROM reservations r
				WHERE r.reservation_type = $1
				AND r.user_id = $2
				"#,
				reservation_type as i32,
				user.id,
			)
			.execute(&mut *transaction)
			.await;

			for reservation in ranges {
				_ = sqlx::query!(
				"INSERT INTO reservations(user_id, reservation_type, range_start, length, time) VALUES($1, $2, $3, $4, $5)",
				reservation.user.id,
				reservation.reservation_type as i32,
				reservation.range_start,
				reservation.length,
				time::PrimitiveDateTime::new(reservation.time.date(), reservation.time.time()),
			)
			.execute(&mut *transaction)
			.await;
			}

			_ = transaction.commit().await;
		}
	}
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Reservation {
	pub user: i64,
	pub reservation_type: ReservationType,
	#[serde(with = "time::serde::rfc3339")]
	pub time: time::OffsetDateTime,
	pub label: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct AllPvs {
	pub reserved_pvs: BTreeMap<i32, Reservation>,
	pub uploaded_pvs: BTreeMap<i32, Vec<Pv>>,
	pub users: BTreeMap<i64, User>,
	pub posts: BTreeMap<i32, Post>,
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_pvs",
	responses(
		(status = 200, body = AllPvs, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn all_pvs(State(state): State<AppState>) -> Result<Json<AllPvs>, (StatusCode, String)> {
	let mut users = BTreeMap::new();
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();

	let mut reserved_pvs = sqlx::query!(
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
					show_explicit: reservation.show_explicit,
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
		"SELECT rl.id, rl.user_id, rl.label FROM reservation_labels rl WHERE rl.reservation_type = $1",
		ReservationType::Song as i32
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	{
		let Some(reservation) = reserved_pvs.get_mut(&record.id) else {
			continue;
		};
		if reservation.user != record.user_id {
			continue;
		};
		reservation.label = Some(record.label.clone());
	}

	let mut uploaded_pvs: BTreeMap<i32, Vec<Pv>> = BTreeMap::new();

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

			if let Some(original) = uploaded_pvs.get_mut(&pv.id) {
				original.push(pv);
			} else {
				uploaded_pvs.insert(pv.id, vec![pv]);
			}
		}
	};

	Ok(Json(AllPvs {
		reserved_pvs,
		uploaded_pvs,
		users,
		posts,
	}))
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct AllModules {
	pub reserved_modules: BTreeMap<i32, Reservation>,
	pub reserved_costumes: BTreeMap<module_db::Chara, BTreeMap<i32, Reservation>>,
	pub uploaded_modules: BTreeMap<i32, Vec<Module>>,
	pub users: BTreeMap<i64, User>,
	pub posts: BTreeMap<i32, Post>,
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_modules",
	responses(
		(status = 200, body = AllModules, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn all_modules(
	State(state): State<AppState>,
) -> Result<Json<AllModules>, (StatusCode, String)> {
	let mut users = BTreeMap::new();
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();

	let mut reserved_modules = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1",
		ReservationType::Module as i32,
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
					show_explicit: reservation.show_explicit,
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

	let mut reserved_costumes = BTreeMap::new();
	for chara in (module_db::Chara::Miku as i32)..=(module_db::Chara::Teto as i32) {
		let module_chara = module_db::Chara::try_from(chara).unwrap();
		reserved_costumes.insert(module_chara, sqlx::query!(
			"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1",
			chara + 10,
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
						show_explicit: reservation.show_explicit
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
		.collect::<BTreeMap<_, _>>());
	}

	for record in sqlx::query!(
		"SELECT * FROM reservation_labels rl WHERE rl.reservation_type != $1 AND rl.reservation_type != $2",
		ReservationType::Song as i32,
		ReservationType::CstmItem as i32,
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	{
		let reservation_type: ReservationType = record.reservation_type.into();
		let reservation = match reservation_type {
			ReservationType::Module => {
				let Some(reservation) = reserved_modules.get_mut(&record.id) else {
					continue;
				};
				reservation
			}
			ReservationType::CosMiku
			| ReservationType::CosRin
			| ReservationType::CosLen
			| ReservationType::CosLuka
			| ReservationType::CosNeru
			| ReservationType::CosHaku
			| ReservationType::CosKaito
			| ReservationType::CosMeiko
			| ReservationType::CosSakine
			| ReservationType::CosTeto => {
				let chara = module_db::Chara::try_from(reservation_type as i32 - 10).unwrap();
				let Some(reservations) = reserved_costumes.get_mut(&chara) else {
					println!("Couldnt get chara {}", chara.to_string());
					continue;
				};
				let Some(reservation) = reservations.get_mut(&record.id) else {
					println!(
						"Couldnt get reservation {}, {}",
						chara.to_string(),
						record.id
					);
					continue;
				};
				reservation
			}
			ReservationType::Song | ReservationType::CstmItem => {
				continue;
			}
		};
		reservation.label = Some(record.label.clone());
	}

	let mut uploaded_modules: BTreeMap<i32, Vec<Module>> = BTreeMap::new();

	if let Ok(search) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("modules"))
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchModule>()
			.await
	{
		for module in search.results {
			let post = if module.post_id == -1 {
				None
			} else if let Some(post) = posts.get(&module.post_id) {
				Some(post.id)
			} else if let Some(post) = Post::get_full(module.post_id, &state.db).await {
				posts.insert(post.id, post.clone());
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

			if let Some(original) = uploaded_modules.get_mut(&module.id) {
				original.push(module);
			} else {
				uploaded_modules.insert(module.id, vec![module]);
			}
		}
	};

	Ok(Json(AllModules {
		reserved_modules,
		reserved_costumes,
		uploaded_modules,
		users,
		posts,
	}))
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct AllCstmItems {
	pub reserved_cstm_items: BTreeMap<i32, Reservation>,
	pub uploaded_cstm_items: BTreeMap<i32, Vec<CstmItem>>,
	pub users: BTreeMap<i64, User>,
	pub posts: BTreeMap<i32, Post>,
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_cstm_items",
	responses(
		(status = 200, body = AllCstmItems, content_type = "application/json"),
		(status = 400, body = String)
	)
)]
pub async fn all_cstm_items(
	State(state): State<AppState>,
) -> Result<Json<AllCstmItems>, (StatusCode, String)> {
	let mut users = BTreeMap::new();
	let mut posts: BTreeMap<i32, Post> = BTreeMap::new();

	let mut reserved_cstm_items = sqlx::query!(
		"SELECT * FROM reservations r LEFT JOIN users u ON r.user_id = u.id WHERE reservation_type = $1",
		ReservationType::CstmItem as i32,
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
					show_explicit: reservation.show_explicit,
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
		"SELECT rl.id, rl.user_id, rl.label FROM reservation_labels rl WHERE rl.reservation_type = $1",
		ReservationType::CstmItem as i32,
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	{
		let Some(reservation) = reserved_cstm_items.get_mut(&record.id) else {
			continue;
		};
		if reservation.user != record.user_id {
			continue;
		};
		reservation.label = Some(record.label.clone());
	}

	let mut uploaded_cstm_items: BTreeMap<i32, Vec<CstmItem>> = BTreeMap::new();

	if let Ok(search) =
		meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("cstm_items"))
			.with_limit(u32::MAX as usize)
			.execute::<MeilisearchCstmItem>()
			.await
	{
		for cstm_item in search.results {
			let post = if cstm_item.post_id == -1 {
				None
			} else if let Some(post) = posts.get(&cstm_item.post_id) {
				Some(post.id)
			} else if let Some(post) = Post::get_full(cstm_item.post_id, &state.db).await {
				posts.insert(post.id, post.clone());
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

			if let Some(original) = uploaded_cstm_items.get_mut(&cstm_item.id) {
				original.push(cstm_item);
			} else {
				uploaded_cstm_items.insert(cstm_item.id, vec![cstm_item]);
			}
		}
	};

	Ok(Json(AllCstmItems {
		reserved_cstm_items,
		uploaded_cstm_items,
		users,
		posts,
	}))
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct AllDbEntries {
	pub entries: BTreeMap<u32, Vec<MeilisearchDbEntry>>,
	pub posts: BTreeMap<i32, Post>,
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_sprite_sets",
	responses(
		(status = 200, body = AllDbEntries, content_type = "application/json"),
		(status = 500, body = String)
	)
)]
pub async fn all_sprite_sets(
	State(state): State<AppState>,
) -> Result<Json<AllDbEntries>, (StatusCode, String)> {
	all_db_entries(String::from("sprite_sets"), state).await
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_sprites",
	responses(
		(status = 200, body = AllDbEntries, content_type = "application/json"),
		(status = 500, body = String)
	)
)]
pub async fn all_sprites(
	State(state): State<AppState>,
) -> Result<Json<AllDbEntries>, (StatusCode, String)> {
	all_db_entries(String::from("sprites"), state).await
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_aet_sets",
	responses(
		(status = 200, body = AllDbEntries, content_type = "application/json"),
		(status = 500, body = String)
	)
)]
pub async fn all_aet_sets(
	State(state): State<AppState>,
) -> Result<Json<AllDbEntries>, (StatusCode, String)> {
	all_db_entries(String::from("aet_sets"), state).await
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_aet_scenes",
	responses(
		(status = 200, body = AllDbEntries, content_type = "application/json"),
		(status = 500, body = String)
	)
)]
pub async fn all_aet_scenes(
	State(state): State<AppState>,
) -> Result<Json<AllDbEntries>, (StatusCode, String)> {
	all_db_entries(String::from("aet_scenes"), state).await
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_objsets",
	responses(
		(status = 200, body = AllDbEntries, content_type = "application/json"),
		(status = 500, body = String)
	)
)]
pub async fn all_objsets(
	State(state): State<AppState>,
) -> Result<Json<AllDbEntries>, (StatusCode, String)> {
	all_db_entries(String::from("objsets"), state).await
}

#[utoipa::path(
	get,
	path = "/api/v1/ids/all_textures",
	responses(
		(status = 200, body = AllDbEntries, content_type = "application/json"),
		(status = 500, body = String)
	)
)]
pub async fn all_textures(
	State(state): State<AppState>,
) -> Result<Json<AllDbEntries>, (StatusCode, String)> {
	all_db_entries(String::from("textures"), state).await
}

pub async fn all_db_entries(
	index: String,
	state: AppState,
) -> Result<Json<AllDbEntries>, (StatusCode, String)> {
	let results = meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index(index))
		.with_limit(u32::MAX as usize)
		.execute::<MeilisearchDbEntry>()
		.await
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

	let mut entries: BTreeMap<u32, Vec<MeilisearchDbEntry>> = BTreeMap::new();
	let mut posts = BTreeMap::new();
	for entry in results.results {
		if !posts.contains_key(&entry.post_id) {
			if let Some(post) = Post::get_full(entry.post_id, &state.db).await {
				posts.insert(post.id, post.clone());
			}
		}
		if let Some(entries) = entries.get_mut(&entry.id) {
			entries.push(entry);
		} else {
			entries.insert(entry.id, vec![entry]);
		}
	}

	Ok(Json(AllDbEntries { entries, posts }))
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
				format!(
					"module_id={} OR (chara={} AND cos.id={})",
					module.id,
					serde_json::to_string(&module.module.chara)
						.unwrap()
						.trim_matches('\"'),
					module.module.cos.id,
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
