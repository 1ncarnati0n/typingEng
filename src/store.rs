// 홈 폴더 JSON 파일에 저장: 통계(~/.typing_eng.json), 보관함(~/.typing_eng_library.json)
use crate::claude::Line;
use crate::youtube::Chunk;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::path::PathBuf;

fn path(name: &str) -> PathBuf {
    std::env::home_dir().unwrap_or_default().join(name)
}

fn load<T: Default + DeserializeOwned>(name: &str) -> T {
    std::fs::read(path(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// 임시 파일에 쓴 뒤 rename — 쓰다가 앱이 꺼져도 기존 파일이 깨지지 않는다.
fn save<T: Serialize>(name: &str, value: &T) {
    let Ok(s) = serde_json::to_string(value) else { return };
    let tmp = path(&format!("{name}.tmp"));
    if std::fs::write(&tmp, s).is_ok() {
        let _ = std::fs::rename(tmp, path(name));
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Stats {
    pub lines_typed: u32,
    pub best_wpm: f64,
    pub missed: HashMap<String, u32>,
    /// 설정: 유튜브 자막을 받을 때 Chrome 로그인 쿠키를 쓰지 않음 (기본은 사용)
    #[serde(default)]
    pub no_yt_cookies: bool,
}

impl Stats {
    pub fn load() -> Self {
        load(".typing_eng.json")
    }
    pub fn save(&self) {
        save(".typing_eng.json", self)
    }
}

/// Claude로 만든 문장 한 묶음.
#[derive(Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub title: String,
    pub lines: Vec<Line>,
}

/// 유튜브 영상: 자막 세트 전체 + Claude로 변환을 마친 세트 + 진행 위치.
#[derive(Clone, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub chunks: Vec<Chunk>,
    /// chunks와 같은 길이. 변환 전이면 None.
    pub sets: Vec<Option<Vec<Line>>>,
    /// 다음에 할 세트 번호
    pub progress: usize,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    pub lessons: Vec<Lesson>,
    pub videos: Vec<Video>,
}

impl Library {
    pub fn load() -> Self {
        load(".typing_eng_library.json")
    }
    pub fn save(&self) {
        save(".typing_eng_library.json", self)
    }
    pub fn video(&self, id: &str) -> Option<&Video> {
        self.videos.iter().find(|v| v.id == id)
    }
    pub fn video_mut(&mut self, id: &str) -> Option<&mut Video> {
        self.videos.iter_mut().find(|v| v.id == id)
    }
}
