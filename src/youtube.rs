// 유튜브 영어 자막을 yt-dlp CLI로 받아 "단어별 시각"으로 만든다. 블로킹이므로 UI 스레드 밖에서 부른다.
use crate::claude::Line;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

/// 한 세트(약 10줄)에 해당하는 단어 수.
pub const CHUNK_WORDS: usize = 120;

/// (영상에서의 시각 초, 단어)
pub type Word = (f64, String);

/// 한 세트 분량의 자막. `end`는 다음 세트가 시작하는 시각.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub words: Vec<Word>,
    pub end: f64,
}

impl Chunk {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.1.as_str()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Debug)]
pub struct Transcript {
    pub id: String,
    pub title: String,
    pub words: Vec<Word>,
    pub end: f64,
}

/// URL에서 영상 ID 추출 (보관함에 이미 있는지 확인용). watch?v=, youtu.be/, shorts/, embed/ 형식.
pub fn video_id(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = ["v=", "youtu.be/", "shorts/", "embed/", "live/"]
        .iter()
        .find_map(|p| url.split_once(p).map(|(_, r)| r))?;
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() == 11).then_some(id)
}

/// 기본 브라우저로 연다.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Command::new(opener).arg(url).spawn().map(|_| ()).map_err(|e| format!("브라우저를 열 수 없습니다: {e}"))
}

/// 자막 하나를 받는다. YouTube 요청을 최소화하는 순서:
/// 1) `yt-dlp -J`로 영상 정보만 받아 자막 목록을 보고 **트랙 하나만** 고른다
/// 2) 받은 정보 파일(`--load-info-json`)을 재사용해 그 트랙만 다운로드 — 페이지를 다시 요청하지 않는다
/// `cookies`면 Chrome에 로그인된 YouTube 쿠키로 요청한다(익명보다 429 제한이 훨씬 느슨함).
pub fn fetch_transcript(url: &str, cookies: bool) -> Result<Transcript, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("유튜브 링크를 입력하세요.".into());
    }
    // 동시에 여러 번 불려도 겹치지 않는 임시 폴더 (시각만으로는 같은 마이크로초에 겹칠 수 있다)
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("typing_eng_yt_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir); // 같은 pid가 재사용된 옛 찌꺼기 정리
    std::fs::create_dir_all(&dir).map_err(|e| format!("임시 폴더 생성 실패: {e}"))?;

    let result = (|| {
        let mut cookies = cookies;
        // "--" 뒤에 URL → '-'로 시작하는 입력이 옵션으로 해석되지 않게
        let info = ytdlp(&dir, &mut cookies, &["-J", "--no-playlist", "--", url])?;
        let v: Value = serde_json::from_slice(&info.stdout).map_err(|_| "영상 정보를 읽을 수 없습니다.".to_string())?;
        let id = v["id"].as_str().unwrap_or_default().to_string();
        let title = v["title"].as_str().unwrap_or(&id).to_string();
        let (flag, lang) = pick_track(&v).ok_or("영어 자막이 없는 영상입니다.")?;
        let info_path = dir.join("info.json");
        std::fs::write(&info_path, &info.stdout).map_err(|e| e.to_string())?;
        let info_arg = info_path.to_string_lossy().to_string();
        ytdlp(
            &dir,
            &mut cookies,
            &["--load-info-json", &info_arg, "--skip-download", flag, "--sub-langs", lang, "--sub-format", "vtt", "-o", "sub"],
        )?;
        let vtt = std::fs::read_to_string(dir.join(format!("sub.{lang}.vtt")))
            .map_err(|_| "자막 파일을 받지 못했습니다. 잠시 후 다시 시도하세요.".to_string())?;
        let (words, end) = parse_vtt(&vtt);
        if words.is_empty() {
            return Err("자막이 비어 있습니다.".into());
        }
        Ok(Transcript { id, title, words, end })
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// 받을 자막 트랙 하나: (yt-dlp 플래그, 언어 코드).
/// 자동 자막 원본(en-orig)을 가장 먼저 — 단어별 시각이 있어 문장 구간이 정확하다. 다음은 사람이 단 영어 자막.
/// ("en.*"처럼 넓게 고르면 "English from Arabic" 같은 자동 번역 트랙 수십 개를 받다가 HTTP 429에 걸린다.)
fn pick_track(info: &Value) -> Option<(&'static str, &'static str)> {
    let has = |kind: &str, lang: &str| info[kind].get(lang).is_some();
    if has("automatic_captions", "en-orig") {
        return Some(("--write-auto-subs", "en-orig"));
    }
    if let Some(l) = ["en", "en-US", "en-GB"].into_iter().find(|l| has("subtitles", l)) {
        return Some(("--write-subs", l));
    }
    has("automatic_captions", "en").then_some(("--write-auto-subs", "en"))
}

/// yt-dlp 실행. 429면 10초·30초 쉬었다가 다시, 쿠키를 못 읽으면 쿠키 없이 다시 시도한다.
fn ytdlp(dir: &std::path::Path, cookies: &mut bool, args: &[&str]) -> Result<std::process::Output, String> {
    let mut waits = [10u64, 30].into_iter();
    loop {
        let mut cmd = Command::new("yt-dlp");
        cmd.current_dir(dir);
        if *cookies {
            cmd.args(["--cookies-from-browser", "chrome"]);
        }
        let out = cmd.args(args).output().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "yt-dlp를 찾을 수 없습니다. 터미널에서 `brew install yt-dlp` 후 다시 시도하세요.".to_string()
            }
            _ => format!("yt-dlp 실행 실패: {e}"),
        })?;
        if out.status.success() {
            return Ok(out);
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if *cookies && err.to_lowercase().contains("cookie") {
            *cookies = false; // Chrome 쿠키를 못 읽음(키체인 거부 등) → 쿠키 없이
            continue;
        }
        if err.contains("429") {
            if let Some(w) = waits.next() {
                std::thread::sleep(std::time::Duration::from_secs(w));
                continue;
            }
            return Err("YouTube가 요청이 너무 많다고 잠시 막았습니다(HTTP 429). 몇 분 뒤 다시 시도하세요. \
                        Chrome에서 YouTube에 로그인한 뒤 '로그인 쿠키 사용'을 켜면 거의 생기지 않습니다."
                .into());
        }
        return Err(format!("yt-dlp 오류: {}", err.trim().lines().last().unwrap_or("")));
    }
}

fn ts(s: &str) -> Option<f64> {
    // "00:01:02.500" 또는 "01:02.500"
    let s = s.trim();
    if !s.contains(':') {
        return None;
    }
    s.split(':').try_fold(0.0, |acc, p| p.parse::<f64>().ok().map(|x| acc * 60.0 + x))
}

/// WebVTT → (단어별 시각, 마지막 큐가 끝나는 시각).
/// 자동 자막은 `<00:00:01.200>` 태그로 단어마다 시각이 있고, 수동 자막은 큐 구간 안에서 단어를 고르게 나눈다.
/// 자동 자막이 굴러가며 반복하는 줄은 건너뛴다.
pub fn parse_vtt(vtt: &str) -> (Vec<Word>, f64) {
    let mut words: Vec<Word> = Vec::new();
    let mut last_line = String::new();
    let (mut cs, mut ce) = (0.0, 0.0);
    let mut pending: Vec<String> = Vec::new(); // 시각 태그 없는 단어 (큐가 끝날 때 고르게 배치)
    let flush = |pending: &mut Vec<String>, words: &mut Vec<Word>, cs: f64, ce: f64| {
        let n = pending.len() as f64;
        for (i, w) in pending.drain(..).enumerate() {
            words.push((cs + (ce - cs) * i as f64 / n, w));
        }
    };
    for line in vtt.lines() {
        let line = line.trim();
        if let Some((a, b)) = line.split_once("-->") {
            flush(&mut pending, &mut words, cs, ce);
            cs = ts(a).unwrap_or(ce);
            ce = b.split_whitespace().next().and_then(ts).unwrap_or(cs);
            continue;
        }
        if line.is_empty()
            || line.chars().all(|c| c.is_ascii_digit())
            || ["WEBVTT", "Kind:", "Language:", "NOTE", "STYLE"].iter().any(|p| line.starts_with(p))
        {
            continue;
        }
        // 태그로 조각내기: (시각, 텍스트)
        let mut pieces: Vec<(Option<f64>, String)> = vec![(None, String::new())];
        let mut rest = line;
        while let Some(open) = rest.find('<') {
            pieces.last_mut().unwrap().1.push_str(&rest[..open]);
            let Some(close) = rest[open..].find('>') else { break };
            if let Some(t) = ts(&rest[open + 1..open + close]) {
                pieces.push((Some(t), String::new()));
            }
            rest = &rest[open + close + 1..];
        }
        if !rest.contains('<') {
            pieces.last_mut().unwrap().1.push_str(rest);
        }
        let pieces: Vec<(Option<f64>, Vec<String>)> = pieces
            .into_iter()
            .map(|(t, s)| {
                let clean = strip_brackets(&decode(&s).replace(">>", " "));
                (t, clean.split_whitespace().map(String::from).collect())
            })
            .collect();
        let joined = pieces.iter().flat_map(|p| p.1.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
        if joined.is_empty() || joined == last_line {
            continue;
        }
        last_line = joined;
        if pieces.iter().any(|p| p.0.is_some()) {
            for (t, ws) in pieces {
                words.extend(ws.into_iter().map(|w| (t.unwrap_or(cs), w)));
            }
        } else {
            // ponytail: 한 큐 안에 태그 있는 줄/없는 줄이 섞이면 순서가 살짝 어긋날 수 있음 — 실제 자막에선 보기 드묾
            pending.extend(pieces.into_iter().flat_map(|p| p.1));
        }
    }
    flush(&mut pending, &mut words, cs, ce);
    (words, ce)
}

/// `[Music]` 같은 대괄호 구간을 지운다.
fn strip_brackets(s: &str) -> String {
    let mut depth = 0;
    s.chars()
        .filter(|&c| {
            match c {
                '[' => depth += 1,
                ']' if depth > 0 => {
                    depth -= 1;
                    return false;
                }
                _ => {}
            }
            depth == 0
        })
        .collect()
}

fn decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// 약 n단어씩 자른다. n±20단어 안에서 문장 끝(. ? !)이 있으면 거기서,
/// 없으면(자동 자막은 구두점이 없다) 말이 가장 오래 끊긴 곳에서 자른다.
pub fn chunks(words: &[Word], end: f64, n: usize) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut stop = words.len();
        if words.len() > i + n + 20 {
            // 남은 게 n+20단어 이하면 자투리 세트를 만들지 않고 한 번에
            let window = (i + n).saturating_sub(20).max(i + 1)..(i + n + 20).min(words.len());
            let sentence_end = window.clone().filter(|&j| words[j - 1].1.ends_with(['.', '?', '!'])).min_by_key(|&j| j.abs_diff(i + n));
            let longest_pause = window.max_by(|&a, &b| (words[a].0 - words[a - 1].0).total_cmp(&(words[b].0 - words[b - 1].0)));
            stop = sentence_end.or(longest_pause).unwrap_or(stop);
        }
        let chunk_end = words.get(stop).map_or(end, |w| w.0);
        out.push(Chunk { words: words[i..stop].to_vec(), end: chunk_end });
        i = stop;
    }
    out
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Claude가 문장으로 정리한 줄마다 영상 구간(start, end)을 붙인다.
/// 원문 단어 순서는 유지된다는 전제로, 각 줄의 첫 단어를 자막 단어에서 가까운 앞쪽부터 찾아 맞춘다.
pub fn align(chunk: &Chunk, lines: &mut [Line]) {
    let words = &chunk.words;
    if words.is_empty() {
        return;
    }
    let mut cursor = 0;
    for line in lines.iter_mut() {
        let lw: Vec<String> = line.en.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
        if !lw.is_empty() {
            // 필러(um, uh) 삭제나 숫자 표기 변경으로 생긴 어긋남을 앞쪽 8단어 / 뒤쪽 3단어 안에서 보정.
            // 흔한 첫 단어(I, and, the…)가 근처에 또 있을 수 있어 첫 두 단어가 맞는 곳을 먼저 찾는다.
            let at = |j: usize, k: usize| words.get(j + k).is_some_and(|w| norm(&w.1) == lw[k]);
            let two = |j: &usize| at(*j, 0) && (lw.len() < 2 || at(*j, 1));
            let one = |j: &usize| at(*j, 0);
            let ahead = || cursor..(cursor + 8).min(words.len());
            let behind = || (cursor.saturating_sub(3)..cursor).rev();
            let found = ahead()
                .find(two)
                .or_else(|| behind().find(two))
                .or_else(|| ahead().find(one))
                .or_else(|| behind().find(one));
            if let Some(j) = found {
                cursor = j;
            }
        }
        line.start = words[cursor.min(words.len() - 1)].0;
        cursor += lw.len();
    }
    for i in 0..lines.len() {
        lines[i].end = lines.get(i + 1).map_or(chunk.end, |l| l.start).max(lines[i].start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(en: &str) -> Line {
        Line { speaker: String::new(), en: en.into(), ko: String::new(), start: 0.0, end: 0.0 }
    }

    #[test]
    fn vtt_auto_captions() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n\
            00:00:00.000 --> 00:00:02.000 align:start position:0%\n \nhello<00:00:00.500><c> there</c>\n\n\
            00:00:02.000 --> 00:00:02.010\nhello there\n \n\n\
            00:00:02.010 --> 00:00:04.000\nhello there\n[Music] it&#39;s &amp; &gt;&gt; fine<00:00:03.000><c> ok</c>\n\n\
            00:00:04.000 --> 00:00:05.000\nit's & fine ok\n";
        let (words, end) = parse_vtt(vtt);
        let w = |t: f64, s: &str| (t, s.to_string());
        assert_eq!(
            words,
            vec![w(0.0, "hello"), w(0.5, "there"), w(2.01, "it's"), w(2.01, "&"), w(2.01, "fine"), w(3.0, "ok")]
        );
        assert_eq!(end, 5.0);
    }

    #[test]
    fn vtt_manual_spreads_words_over_cue() {
        let vtt = "WEBVTT\n\n1\n01:00:00.000 --> 01:00:04.000\nOne two.\nThree four.\n\n2\n01:00:04.000 --> 01:00:06.000\nFive.\n";
        let (words, end) = parse_vtt(vtt);
        let times: Vec<f64> = words.iter().map(|w| w.0).collect();
        assert_eq!(times, vec![3600.0, 3601.0, 3602.0, 3603.0, 3604.0]);
        assert_eq!(words[4].1, "Five.");
        assert_eq!(end, 3606.0);
    }

    #[test]
    fn chunk_boundaries() {
        // 구두점이 있으면 n에 가장 가까운 문장 끝
        let words: Vec<Word> =
            (1..=200).map(|i| (i as f64, if i == 125 { "end.".to_string() } else { format!("w{i}") })).collect();
        let c = chunks(&words, 210.0, 120);
        assert_eq!(c.len(), 2);
        assert!(c[0].text().ends_with("end.") && c[0].words[0].0 == 1.0 && c[0].end == 126.0);
        assert_eq!(c[1].words.len(), 75);
        assert_eq!(c[1].end, 210.0);
        // 구두점이 없으면(자동 자막) 가장 긴 쉼 앞에서: 110번째 단어 다음에 3초 쉼
        let words: Vec<Word> =
            (0..200).map(|i| (i as f64 + if i >= 110 { 3.0 } else { 0.0 }, format!("w{i}"))).collect();
        let c = chunks(&words, 300.0, 120);
        assert_eq!(c[0].words.len(), 110);
        // 자투리는 붙인다
        assert_eq!(chunks(&words[..130], 300.0, 120).len(), 1);
        assert!(chunks(&[], 0.0, 5).is_empty());
    }

    #[test]
    fn align_lines_to_words() {
        let words: Vec<Word> = "so um I went to the store and then we had uh 20 apples"
            .split(' ')
            .enumerate()
            .map(|(i, w)| (i as f64, w.to_string()))
            .collect();
        let chunk = Chunk { words, end: 20.0 };
        let mut lines = vec![line("So I went to the store."), line("And then we had 20 apples.")];
        align(&chunk, &mut lines);
        assert_eq!((lines[0].start, lines[0].end), (0.0, 7.0));
        assert_eq!((lines[1].start, lines[1].end), (7.0, 20.0));
    }

    #[test]
    fn align_prefers_two_word_match() {
        // 둘째 줄 "I think so."의 첫 단어 I가 바로 앞(인덱스 4)에도 있다
        let words: Vec<Word> = "you know what um I I think so"
            .split(' ')
            .enumerate()
            .map(|(i, w)| (i as f64, w.to_string()))
            .collect();
        let chunk = Chunk { words, end: 10.0 };
        let mut lines = vec![line("You know what, I."), line("I think so.")];
        align(&chunk, &mut lines);
        assert_eq!(lines[1].start, 5.0);
        assert_eq!(lines[0].end, 5.0);
    }

    #[test]
    fn picks_one_track() {
        let info = |v: Value| pick_track(&v);
        let both = serde_json::json!({"automatic_captions": {"en-orig": [], "en": [], "en-ar": []}, "subtitles": {"en": []}});
        assert_eq!(info(both), Some(("--write-auto-subs", "en-orig")));
        let manual = serde_json::json!({"automatic_captions": {"en": []}, "subtitles": {"en-US": []}});
        assert_eq!(info(manual), Some(("--write-subs", "en-US")));
        assert_eq!(info(serde_json::json!({"automatic_captions": {"en": []}})), Some(("--write-auto-subs", "en")));
        assert_eq!(info(serde_json::json!({"automatic_captions": {"en-ar": []}, "subtitles": {"ko": []}})), None);
    }

    #[test]
    fn ids() {
        assert_eq!(video_id("https://www.youtube.com/watch?v=jNQXAC9IVRw&t=3s").as_deref(), Some("jNQXAC9IVRw"));
        assert_eq!(video_id("https://youtu.be/jNQXAC9IVRw?si=x").as_deref(), Some("jNQXAC9IVRw"));
        assert_eq!(video_id("https://example.com"), None);
    }

    // 실제 네트워크 + yt-dlp + claude CLI(사용량 소모). 자막 → 세트 → Claude 문장화 → 구간 맞추기 전체 확인.
    #[test]
    #[ignore]
    fn live_pipeline() {
        let t = fetch_transcript("https://www.youtube.com/watch?v=arj7oStGLkU", false).unwrap();
        assert!(t.words.windows(2).all(|w| w[0].0 <= w[1].0), "단어 시각이 거꾸로 감");
        let cs = chunks(&t.words, t.end, CHUNK_WORDS);
        let mut lines = crate::claude::lines_from_transcript(crate::claude::Engine::Claude, &cs[0].text()).unwrap();
        align(&cs[0], &mut lines);
        for l in &lines {
            eprintln!("{:6.2}-{:6.2} {}", l.start, l.end, l.en);
        }
        assert!(lines.windows(2).all(|w| w[0].start <= w[1].start), "문장 시작이 거꾸로 감");
        assert!(lines.iter().all(|l| l.start < l.end && l.start >= cs[0].words[0].0 && l.end <= cs[0].end));
    }

    // 실제 네트워크 + yt-dlp 필요. `cargo test -- --ignored` 로만 실행.
    #[test]
    #[ignore]
    fn live_fetch() {
        let t = fetch_transcript("https://www.youtube.com/watch?v=jNQXAC9IVRw", false).unwrap();
        assert_eq!(t.id, "jNQXAC9IVRw");
        assert!(!t.title.is_empty());
        assert!(t.words.len() > 5, "{:?}", t.words);
    }
}
