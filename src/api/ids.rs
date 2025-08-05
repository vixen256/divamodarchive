use crate::AppState;
use crate::models::*;
use axum::{extract::*, http::StatusCode, response::*};
use base64::prelude::*;
use itertools::*;
use serde::{Deserialize, Serialize};
use std::collections::*;
use std::path::Path;
use tokio::process::Command;

#[derive(Serialize, Deserialize, Clone)]
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
pub struct MeilisearchNcSong {
	pub uid: u64,
	pub post_id: i32,
	pub pv_id: i32,
	pub difficulties: [Option<MeilisearchNcDifficulty>; 5],
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MeilisearchNcDifficulty {
	pub arcade: Option<MeilisearchNcChart>,
	pub console: Option<MeilisearchNcChart>,
	pub mixed: Option<MeilisearchNcChart>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MeilisearchNcChart {
	// If is none its inhereted from the songs existing pv_db
	pub level: Option<pv_db::Level>,
}

#[derive(Serialize, Deserialize)]
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

	let mut entries = Vec::new();
	for (id, set) in spr_db.sets {
		entries.push(MeilisearchDbEntry {
			uid: (post_id as u64) << 32 | (id as u64),
			post_id,
			id,
			name: set.name,
		});

		for (id, sprite) in set.sprites {
			entries.push(MeilisearchDbEntry {
				uid: (post_id as u64) << 32 | (id as u64),
				post_id,
				id,
				name: sprite.name,
			});
		}

		for (id, texture) in set.textures {
			entries.push(MeilisearchDbEntry {
				uid: (post_id as u64) << 32 | (id as u64),
				post_id,
				id,
				name: texture.name,
			});
		}
	}

	let base = meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("sprites"))
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
		.index("sprites")
		.add_or_update(&entries, Some("uid"))
		.await
		.ok()?;

	Some(())
}

async fn parse_aet_db<P: AsRef<Path>>(path: P, post_id: i32, state: &AppState) -> Option<()> {
	let aet_db = diva_db::AetDb::from_file(path).ok()?;

	let mut entries = Vec::new();
	for (id, set) in aet_db.sets {
		entries.push(MeilisearchDbEntry {
			uid: (post_id as u64) << 32 | (id as u64),
			post_id,
			id,
			name: set.name,
		});

		for (id, scene) in set.scenes {
			entries.push(MeilisearchDbEntry {
				uid: (post_id as u64) << 32 | (id as u64),
				post_id,
				id,
				name: scene.name,
			});
		}
	}

	let base = meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("aets"))
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
		.index("aets")
		.add_or_update(&entries, Some("uid"))
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

	pub fn owns_nc(&self, nc_songs: &[NcSong]) -> bool {
		let Some(post) = self.post else { return false };

		nc_songs.iter().any(|nc_song| nc_song.post == post)
	}
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Module {
	pub uid: String,
	pub post: Option<i32>,
	pub id: i32,
	pub module: module_db::Module,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CstmItem {
	pub uid: String,
	pub post: Option<i32>,
	pub id: i32,
	pub cstm_item: module_db::CustomizeItem,
}

#[derive(Serialize, Deserialize, Default)]
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

#[derive(Serialize, Deserialize, Default)]
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
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

	let mut hits = pvs.hits.into_iter().map(|p| p.result).collect::<Vec<_>>();

	while pvs.estimated_total_hits.unwrap_or(0) > hits.len()
		&& query.limit.unwrap_or(u32::MAX as usize) > hits.len()
	{
		search.offset = Some(hits.len() + query.offset.unwrap_or(0));
		pvs = search
			.execute::<MeilisearchPv>()
			.await
			.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

	let mut nc_songs: BTreeMap<i32, Vec<NcSong>> = BTreeMap::new();
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

	Ok(Json(PvSearch {
		pvs: vec,
		nc_songs,
		posts,
	}))
}

#[derive(Serialize, Deserialize, Default)]
pub struct ModuleSearch {
	pub modules: Vec<Module>,
	pub posts: BTreeMap<i32, Post>,
}

impl ModuleSearch {
	pub fn find_module(&self, id: i32) -> Option<&Module> {
		self.modules.iter().find(|module| module.id == id)
	}
}

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
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
			.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

#[derive(Serialize, Deserialize, Default)]
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
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
			.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

#[derive(Serialize, Deserialize, Default)]
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
		.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
			.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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

	let filter = vec
		.iter()
		.map(|pv| format!("pv_id={}", pv.pv_id))
		.intersperse(String::from(" OR "))
		.collect::<String>();

	let search = meilisearch_sdk::documents::DocumentsQuery::new(&state.meilisearch.index("pvs"))
		.with_limit(u32::MAX as usize)
		.with_filter(&filter)
		.execute::<MeilisearchPv>()
		.await;

	let mut pvs: BTreeMap<i32, Vec<Pv>> = BTreeMap::new();
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

	Ok(Json(NcSongSearch {
		nc_songs: vec,
		pvs,
		posts,
	}))
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
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

	_ = sqlx::query!(
		r#"
		UPDATE reservations r
		SET time = '2000-01-01'
		WHERE r.reservation_type = $1
		AND r.user_id = $2
		AND (r.range_start = $3 OR r.range_start + r.length > $3) AND r.range_start < $4
		"#,
		query.reservation_type as i32,
		user.id,
		query.start,
		(query.start + query.length)
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
		AND (r.range_start = $3 OR r.range_start + r.length > $3) AND r.range_start < $4
		"#,
		query.reservation_type as i32,
		user.id,
		query.start,
		(query.start + query.length)
	)
	.execute(&state.db)
	.await;
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

		_ = sqlx::query!("BEGIN WORK").execute(&state.db).await;

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
				"INSERT INTO reservations(user_id, reservation_type, range_start, length, time) VALUES($1, $2, $3, $4, $5)",
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

		_ = sqlx::query!("COMMIT WORK").execute(&state.db).await;
	}
}

#[derive(Serialize, Deserialize)]
pub struct Reservation {
	pub user: i64,
	pub reservation_type: ReservationType,
	#[serde(with = "time::serde::rfc3339")]
	pub time: time::OffsetDateTime,
	pub label: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct AllPvs {
	pub reserved_pvs: BTreeMap<i32, Reservation>,
	pub uploaded_pvs: BTreeMap<i32, Vec<Pv>>,
	pub users: BTreeMap<i64, User>,
	pub posts: BTreeMap<i32, Post>,
}

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

#[derive(Serialize, Deserialize)]
pub struct AllModules {
	pub reserved_modules: BTreeMap<i32, Reservation>,
	pub uploaded_modules: BTreeMap<i32, Vec<Module>>,
	pub users: BTreeMap<i64, User>,
	pub posts: BTreeMap<i32, Post>,
}

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
		ReservationType::Module as i32
	)
	.fetch_all(&state.db)
	.await
	.unwrap_or_default()
	{
		let Some(reservation) = reserved_modules.get_mut(&record.id) else {
			continue;
		};
		if reservation.user != record.user_id {
			continue;
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
		uploaded_modules,
		users,
		posts,
	}))
}

#[derive(Serialize, Deserialize)]
pub struct AllCstmItems {
	pub reserved_cstm_items: BTreeMap<i32, Reservation>,
	pub uploaded_cstm_items: BTreeMap<i32, Vec<CstmItem>>,
	pub users: BTreeMap<i64, User>,
	pub posts: BTreeMap<i32, Post>,
}

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
