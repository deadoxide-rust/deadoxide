//! Launcher logic without any UI: registry types, download + sha256 + safe extract, game discovery.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const SITE: &str = "https://deadoxide-rust.github.io/deadoxide/";

#[derive(Deserialize, Clone, Debug)]
pub struct Mod {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub url: String,
    pub sha256: String,
}
#[derive(Deserialize)]
pub struct Registry {
    pub mods: Vec<Mod>,
}
#[derive(Deserialize, Clone)]
pub struct Area {
    pub name: String,
    pub percent: f32,
    pub measured: bool,
}
#[derive(Deserialize, Clone)]
pub struct Progress {
    pub overall: f32,
    pub generated: String,
    pub areas: Vec<Area>,
}

#[derive(Deserialize, Clone)]
pub struct Release {
    pub version: String,
    pub date: String,
    #[serde(default)]
    pub new: Vec<String>,
    #[serde(default)]
    pub fixed: Vec<String>,
}
#[derive(Deserialize)]
pub struct Changelog {
    pub releases: Vec<Release>,
}

pub fn get_bytes(url: &str) -> Result<Vec<u8>, String> {
    if let Some(p) = url.strip_prefix("file:") {
        let mut v = std::fs::read(p).map_err(|e| e.to_string())?;
        if v.starts_with(&[0xEF, 0xBB, 0xBF]) {
            v.drain(..3);
        }
        return Ok(v);
    }
    let mut r = ureq::get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut v = Vec::new();
    r.body_mut()
        .as_reader()
        .take(64 << 20)
        .read_to_end(&mut v)
        .map_err(|e| e.to_string())?;
    if v.starts_with(&[0xEF, 0xBB, 0xBF]) {
        v.drain(..3);
    }
    Ok(v)
}

/// The rule of `zip`'s own `ZipFileData::enclosed_name` (2.6.1), applied to a
/// name whose Windows separators have been turned into `/`: the path is
/// returned only when it cannot escape the directory it is later joined to.
fn enclosed_path(name: &str) -> Option<PathBuf> {
    if name.contains('\0') {
        return None;
    }
    let path = PathBuf::from(name);
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return None,
            Component::ParentDir => depth = depth.checked_sub(1)?,
            Component::Normal(_) => depth += 1,
            Component::CurDir => (),
        }
    }
    Some(path)
}

/// Verify sha256, then extract. Entries that would escape `dest` (zip-slip) are rejected.
pub fn install(m: &Mod, base: &str, mods_dir: &Path) -> Result<PathBuf, String> {
    let url = if m.url.contains("://") || m.url.starts_with("file:") {
        m.url.clone()
    } else {
        format!("{base}{}", m.url)
    };
    let data = get_bytes(&url)?;
    let got: String = Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !got.eq_ignore_ascii_case(&m.sha256) {
        return Err(format!("sha256 mismatch: expected {}, got {got}", m.sha256));
    }
    let dest = mods_dir.join(&m.id);
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        // Archives built on Windows store `\\` as the separator, which on Linux
        // is an ordinary file name character: normalize before resolving it.
        let rel = enclosed_path(&f.name().replace('\\', "/"))
            .ok_or_else(|| format!("unsafe path in archive: {}", f.name()))?;
        let out = dest.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut w).map_err(|e| e.to_string())?;
        }
    }
    Ok(dest)
}

pub fn find_game() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = vec![];
    if let Ok(p) = std::env::var("PZ_ROOT") {
        c.push(p.into());
    }
    let mut d = std::env::current_dir().ok();
    while let Some(p) = d {
        c.push(p.clone());
        d = p.parent().map(Path::to_path_buf);
    }
    let mut steam: Vec<PathBuf> = [
        "C:/Program Files (x86)/Steam",
        "C:/Program Files/Steam",
        "D:/Steam",
        "D:/SteamLibrary",
        "E:/Games",
        "E:/SteamLibrary",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = std::env::var_os("HOME") {
        steam.extend(linux_steam_roots(Path::new(&home)));
    }
    for root in steam {
        c.extend(game_dirs(&root));
    }
    c.into_iter()
        .find(|p| p.join("projectzomboid.jar").exists())
}

/// Linux Steam installs: native (both the XDG path and the legacy `~/.steam` links) and Flatpak,
/// plus every extra library listed in each one's `libraryfolders.vdf`.
pub fn linux_steam_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![];
    for base in [
        ".local/share/Steam",
        ".steam/steam",
        ".steam/root",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    ] {
        let root = home.join(base);
        let vdf = root.join("steamapps/libraryfolders.vdf");
        roots.push(root);
        if let Ok(text) = std::fs::read_to_string(vdf) {
            roots.extend(library_paths(&text));
        }
    }
    roots
}

/// The `"path"` values of a Steam `libraryfolders.vdf`.
pub fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut quoted = line.split('"').skip(1).step_by(2);
            (quoted.next() == Some("path"))
                .then(|| quoted.next())
                .flatten()
                .map(|p| PathBuf::from(p.replace("\\\\", "\\")))
        })
        .collect()
}

/// Where the game lives inside a Steam library. The Linux depot puts the files one level
/// deeper, in `ProjectZomboid/projectzomboid`.
pub fn game_dirs(library: &Path) -> [PathBuf; 2] {
    let game = library.join("steamapps/common/ProjectZomboid");
    [game.join("projectzomboid"), game]
}

pub fn find_client() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "zomboid-client.exe"
    } else {
        "zomboid-client"
    };
    let mut c = vec![];
    if let Ok(e) = std::env::current_exe() {
        if let Some(d) = e.parent() {
            c.push(d.join(name));
        }
    }
    c.push(PathBuf::from("target/release").join(name));
    c.push(PathBuf::from("zomboid-rs/target/release").join(name));
    c.into_iter().find(|p| p.exists())
}

pub fn find_players_db(dir: &Path) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_players_db(&p) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == "players.db") {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> String {
        format!(
            "file:{}/../site/",
            env!("CARGO_MANIFEST_DIR").replace(std::path::MAIN_SEPARATOR, "/")
        )
    }
    fn catalog_mod() -> Mod {
        let b = get_bytes(&format!("{}registry.json", site())).unwrap();
        serde_json::from_slice::<Registry>(&b)
            .unwrap()
            .mods
            .remove(0)
    }

    #[test]
    fn installs_the_catalog_mod_into_a_fresh_dir() {
        let dir = std::env::temp_dir().join(format!("deadoxide-test-{}", std::process::id()));
        let out = install(&catalog_mod(), &site(), &dir).unwrap();
        let lua = std::fs::read_to_string(out.join("media/lua/client/HelloDeadOxide.lua")).unwrap();
        assert!(lua.contains("mod loaded"));
        assert!(out.join("mod.info").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn archive(id: &str, entries: &[(&str, &[u8])]) -> (Mod, PathBuf) {
        use std::io::Write as _;
        let dir =
            std::env::temp_dir().join(format!("deadoxide-archive-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (entry, body) in entries {
            z.start_file(*entry, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(body).unwrap();
        }
        let bytes = z.finish().unwrap().into_inner();
        let file = dir.join("mod.zip");
        std::fs::write(&file, &bytes).unwrap();
        let sha256 = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let m = Mod {
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            author: "test".into(),
            description: String::new(),
            url: format!("file:{}", file.display()),
            sha256,
        };
        (m, dir.join("mods"))
    }

    #[test]
    fn windows_separators_become_directories() {
        let (m, mods) = archive(
            "backslash",
            &[
                ("media\\lua\\client\\HelloDeadOxide.lua", b"mod loaded"),
                ("mod.info", b"name=Hello"),
            ],
        );
        let out = install(&m, "file:", &mods).unwrap();
        let lua = std::fs::read_to_string(out.join("media/lua/client/HelloDeadOxide.lua")).unwrap();
        assert!(lua.contains("mod loaded"));
        assert!(!out.join("media\\lua\\client\\HelloDeadOxide.lua").exists());
        let _ = std::fs::remove_dir_all(&mods);
    }

    #[test]
    fn traversal_is_rejected() {
        let (m, mods) = archive("traversal", &[("../escape.lua", b"x")]);
        let err = install(&m, "file:", &mods).unwrap_err();
        assert!(err.contains("unsafe path"), "{err}");
        assert!(!mods.join("escape.lua").exists());
        let _ = std::fs::remove_dir_all(&mods);
    }

    #[test]
    fn parent_dir_that_stays_inside_still_resolves() {
        let (m, mods) = archive("inside", &[("media/../mod.info", b"name=Hello")]);
        let out = install(&m, "file:", &mods).unwrap();
        assert!(out.join("mod.info").is_file());
        let _ = std::fs::remove_dir_all(&mods);
    }

    #[test]
    fn wrong_sha256_installs_nothing() {
        let dir = std::env::temp_dir().join(format!("deadoxide-test-bad-{}", std::process::id()));
        let mut m = catalog_mod();
        m.sha256 = "0".repeat(64);
        assert!(install(&m, &site(), &dir).unwrap_err().contains("sha256"));
        assert!(!dir.exists());
    }

    #[test]
    fn reads_steam_library_paths() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/.local/share/Steam\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"108600\"\t\t\"0\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            library_paths(vdf),
            [
                PathBuf::from("/home/u/.local/share/Steam"),
                PathBuf::from("D:\\SteamLibrary")
            ]
        );
    }

    #[test]
    fn finds_the_linux_depot_in_an_extra_flatpak_library() {
        let home = std::env::temp_dir().join(format!("deadoxide-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let library = home.join("games");
        let flatpak = home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps");
        std::fs::create_dir_all(&flatpak).unwrap();
        std::fs::write(
            flatpak.join("libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                library.display()
            ),
        )
        .unwrap();
        let game = library.join("steamapps/common/ProjectZomboid/projectzomboid");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("projectzomboid.jar"), b"").unwrap();
        let found = linux_steam_roots(&home)
            .iter()
            .flat_map(|r| game_dirs(r))
            .find(|p| p.join("projectzomboid.jar").exists());
        assert_eq!(found, Some(game));
        let _ = std::fs::remove_dir_all(&home);
    }
}
