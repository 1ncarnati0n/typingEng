// 유튜브 영어 자막을 yt-dlp CLI로 받아 평문으로 만든다. 블로킹이므로 UI 스레드 밖에서 부른다.
use std::process::Command;

/// 한 세트(약 10줄)에 해당하는 단어 수.
pub const CHUNK_WORDS: usize = 120;

/// 한 세트 분량의 자막과 영상에서의 시작 시각(초).
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub start: u32,
    pub text: String,
}

/// 해당 시각부터 재생되는 유튜브 링크.
pub fn watch_url(id: &str, secs: u32) -> String {
    format!("https://www.youtube.com/watch?v={id}&t={secs}s")
}

/// 기본 브라우저로 연다.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Command::new(opener).arg(url).spawn().map(|_| ()).map_err(|e| format!("브라우저를 열 수 없습니다: {e}"))
}

/// (영상 ID, 시각별 자막 줄). 수동 자막이 없으면 자동 생성 자막을 쓴다.
pub fn fetch_transcript(url: &str) -> Result<(String, Vec<(u32, String)>), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("유튜브 링크를 입력하세요.".into());
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("typing_eng_yt_{nanos}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("임시 폴더 생성 실패: {e}"))?;

    let out = Command::new("yt-dlp")
        .current_dir(&dir)
        .args(["--skip-download", "--write-subs", "--write-auto-subs"])
        .args(["--sub-langs", "en.*", "--sub-format", "vtt", "--no-playlist", "-o", "%(id)s"])
        .arg("--") // URL이 '-'로 시작해도 옵션으로 해석되지 않게
        .arg(url)
        .output();

    let result = (|| {
        let out = out.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                "yt-dlp를 찾을 수 없습니다. 터미널에서 `brew install yt-dlp` 후 다시 시도하세요.".to_string()
            }
            _ => format!("yt-dlp 실행 실패: {e}"),
        })?;
        // 수동 자막(en, en-US…)을 자동 자막 원본(en-orig)보다 우선
        let mut vtts: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "vtt"))
            .collect();
        vtts.sort_by_key(|p| (p.to_string_lossy().contains("orig"), p.clone()));
        let Some(path) = vtts.first() else {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(if out.status.success() {
                "영어 자막이 없는 영상입니다.".to_string()
            } else {
                format!("자막을 가져오지 못했습니다: {}", err.trim().lines().last().unwrap_or(""))
            });
        };
        // 파일 이름이 "<영상ID>.<언어>.vtt" — 영상 ID에는 '.'이 없다
        let id = path.file_name().unwrap_or_default().to_string_lossy().split('.').next().unwrap_or("").to_string();
        let segs = parse_vtt(&std::fs::read_to_string(path).map_err(|e| e.to_string())?);
        if segs.is_empty() {
            return Err("자막이 비어 있습니다.".into());
        }
        Ok((id, segs))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// WebVTT → (큐 시작 초, 텍스트) 목록. 자동 자막의 단어별 타이밍 태그와 굴러가며 반복되는 줄을 제거한다.
pub fn parse_vtt(vtt: &str) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    let mut at = 0;
    for line in vtt.lines() {
        let line = line.trim();
        if let Some((start, _)) = line.split_once("-->") {
            // "00:01:02.500" 또는 "01:02.500"
            at = start.trim().split(':').fold(0.0, |acc, p| acc * 60.0 + p.parse::<f64>().unwrap_or(0.0)) as u32;
            continue;
        }
        if line.is_empty()
            || line.chars().all(|c| c.is_ascii_digit())
            || ["WEBVTT", "Kind:", "Language:", "NOTE", "STYLE"].iter().any(|p| line.starts_with(p))
        {
            continue;
        }
        let clean = decode(&strip(line, '<', '>')).replace(">>", " ");
        let clean = strip(&clean, '[', ']').split_whitespace().collect::<Vec<_>>().join(" ");
        if !clean.is_empty() && out.last().map(|(_, t)| t) != Some(&clean) {
            out.push((at, clean));
        }
    }
    out
}

/// open..close 구간을 지운다 (`<c>`, `[Music]` 등).
fn strip(s: &str, open: char, close: char) -> String {
    let mut depth = 0;
    s.chars()
        .filter(|&c| {
            if c == open {
                depth += 1;
            } else if c == close && depth > 0 {
                depth -= 1;
                return false;
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

/// 약 n단어씩 자른다. 뒤 20단어 안에 문장 끝(. ? !)이 있으면 거기서 자른다.
// ponytail: 자동 자막은 구두점이 없어 문장 중간에서 잘릴 수 있음 — 거슬리면 Claude가 남은 조각을 돌려주게 확장
pub fn chunks(segs: &[(u32, String)], n: usize) -> Vec<Chunk> {
    let words: Vec<(u32, &str)> =
        segs.iter().flat_map(|(t, s)| s.split_whitespace().map(move |w| (*t, w))).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut end = (i + n).min(words.len());
        if let Some(j) = (end..(end + 20).min(words.len()))
            .find(|&j| words[j - 1].1.ends_with(['.', '?', '!']))
        {
            end = j;
        }
        let text = words[i..end].iter().map(|(_, w)| *w).collect::<Vec<_>>().join(" ");
        out.push(Chunk { start: words[i].0, text });
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vtt_auto_captions() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n\
            00:00:00.000 --> 00:00:02.000 align:start position:0%\n \nhello<00:00:00.500><c> there</c>\n\n\
            00:00:02.000 --> 00:00:02.010\nhello there\n \n\n\
            00:00:02.010 --> 00:00:04.000\nhello there\n[Music] it&#39;s &amp; &gt;&gt; fine<00:00:03.000><c> ok</c>\n\n\
            1\n01:00:04.000 --> 01:00:05.000\nit's & fine ok\nnew line\n";
        let segs = parse_vtt(vtt);
        assert_eq!(
            segs,
            vec![(0, "hello there".into()), (2, "it's & fine ok".into()), (3604, "new line".into())]
        );
    }

    #[test]
    fn chunk_boundaries() {
        // 한 줄에 한 단어, i초에 등장
        let segs: Vec<(u32, String)> =
            (1..=130).map(|i| (i, if i == 125 { "end.".to_string() } else { format!("w{i}") })).collect();
        let c = chunks(&segs, 120);
        assert_eq!(c.len(), 2);
        assert!(c[0].text.ends_with("end.") && c[0].start == 1);
        assert_eq!(c[1], Chunk { start: 126, text: "w126 w127 w128 w129 w130".into() });
        let c = chunks(&[(5, "a b".into()), (9, "c".into())], 2);
        assert_eq!(c.iter().map(|c| c.start).collect::<Vec<_>>(), vec![5, 9]);
        assert!(chunks(&[], 5).is_empty());
        assert_eq!(watch_url("abc", 62), "https://www.youtube.com/watch?v=abc&t=62s");
    }

    // 실제 네트워크 + yt-dlp 필요. `cargo test -- --ignored` 로만 실행.
    #[test]
    #[ignore]
    fn live_fetch() {
        let (id, segs) = fetch_transcript("https://www.youtube.com/watch?v=jNQXAC9IVRw").unwrap();
        assert_eq!(id, "jNQXAC9IVRw");
        assert!(segs.len() > 2, "{segs:?}");
    }
}
