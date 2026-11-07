mod claude;
mod typing;
mod youtube;

use claude::{Line, Review};
use iced::widget::{
    Column, button, column, container, operation, rich_text, row, rule, scrollable, span, text,
    text_editor, text_input,
};
use iced::{Center, Color, Element, Fill, Font, Task, Theme, color};
use std::collections::HashMap;
use std::time::Instant;
use typing::CharState;

const LEVELS: [&str; 4] = ["A1", "A2", "B1", "B2"];
const INPUT_ID: &str = "typing";
const GREEN: Color = color!(0x9ece6a);
const RED: Color = color!(0xf7768e);
const MUTED: Color = color!(0x737aa2);
const ACCENT: Color = color!(0x7aa2f7);

// ---- 저장되는 통계 (~/.typing_eng.json) ----

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Stats {
    lines_typed: u32,
    best_wpm: f64,
    missed: HashMap<String, u32>,
}

fn stats_path() -> std::path::PathBuf {
    std::env::home_dir().unwrap_or_default().join(".typing_eng.json")
}

impl Stats {
    fn load() -> Self {
        std::fs::read(stats_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(stats_path(), s);
        }
    }
}

// ---- 상태 ----

#[derive(PartialEq)]
enum Screen {
    Setup,
    Typing,
    Writing,
}

struct App {
    stats: Stats,
    screen: Screen,
    level: &'static str,
    chat: bool,
    topic: String,
    lines: Vec<Line>,
    idx: usize,
    typed: String,
    line_start: Option<Instant>,
    total_secs: f64,
    total_correct: usize,
    total_chars: usize,
    editor: text_editor::Content,
    review: Option<Review>,
    busy: bool,
    error: Option<String>,
    yt_url: String,
    yt_id: String,
    yt_chunks: Vec<youtube::Chunk>,
    yt_pos: usize,
}

#[derive(Debug, Clone)]
enum Msg {
    Level(&'static str),
    Chat(bool),
    Topic(String),
    Generate,
    Generated(Result<Vec<Line>, String>),
    Typed(String),
    Submit,
    Edit(text_editor::Action),
    AskReview,
    Reviewed(Result<Review, String>),
    Home,
    YtUrl(String),
    YtLoad,
    YtLoaded(Result<(String, Vec<youtube::Chunk>), String>),
    NextChunk,
    OpenVideo,
}

/// claude CLI 호출은 블로킹이라 별도 스레드에서 돌리고 결과만 future로 받는다.
fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl Future<Output = T> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    async move { rx.await.expect("worker thread panicked") }
}

impl App {
    fn new() -> Self {
        Self {
            stats: Stats::load(),
            screen: Screen::Setup,
            level: "A2",
            chat: true,
            topic: "ordering coffee".into(),
            lines: vec![],
            idx: 0,
            typed: String::new(),
            line_start: None,
            total_secs: 0.0,
            total_correct: 0,
            total_chars: 0,
            editor: text_editor::Content::new(),
            review: None,
            busy: false,
            error: None,
            yt_url: String::new(),
            yt_id: String::new(),
            yt_chunks: vec![],
            yt_pos: 0,
        }
    }

    fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Level(l) => self.level = l,
            Msg::Chat(c) => self.chat = c,
            Msg::Topic(t) => self.topic = t,
            Msg::Generate => {
                self.busy = true;
                self.error = None;
                self.yt_chunks.clear();
                let (level, chat, topic) = (self.level, self.chat, self.topic.clone());
                return Task::perform(
                    off_thread(move || claude::generate_lines(level, chat, &topic)),
                    Msg::Generated,
                );
            }
            Msg::Generated(res) => {
                self.busy = false;
                match res {
                    Ok(lines) => {
                        self.lines = lines;
                        self.idx = 0;
                        self.typed.clear();
                        self.line_start = None;
                        (self.total_secs, self.total_correct, self.total_chars) = (0.0, 0, 0);
                        self.editor = text_editor::Content::new();
                        self.review = None;
                        self.screen = Screen::Typing;
                        return operation::focus(INPUT_ID);
                    }
                    Err(e) => {
                        // 유튜브 세트 변환이 실패하면 같은 세트를 다시 시도할 수 있게 되돌린다
                        self.yt_pos = self.yt_pos.saturating_sub(1);
                        self.error = Some(e);
                    }
                }
            }
            Msg::Typed(s) => {
                if self.line_start.is_none() && !s.is_empty() {
                    self.line_start = Some(Instant::now());
                }
                self.typed = s;
            }
            Msg::Submit => {
                if self.typed.chars().count() >= self.lines[self.idx].en.chars().count() {
                    self.finish_line();
                }
            }
            Msg::Edit(a) => self.editor.perform(a),
            Msg::AskReview => {
                self.busy = true;
                self.error = None;
                let (lines, written) = (self.lines.clone(), self.editor.text());
                return Task::perform(
                    off_thread(move || claude::review_writing(&lines, &written)),
                    Msg::Reviewed,
                );
            }
            Msg::Reviewed(res) => {
                self.busy = false;
                match res {
                    Ok(r) => self.review = Some(r),
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::YtUrl(u) => self.yt_url = u,
            Msg::YtLoad => {
                self.busy = true;
                self.error = None;
                let url = self.yt_url.clone();
                return Task::perform(
                    off_thread(move || {
                        youtube::fetch_transcript(&url).map(|(id, segs)| (id, youtube::chunks(&segs, youtube::CHUNK_WORDS)))
                    }),
                    Msg::YtLoaded,
                );
            }
            Msg::YtLoaded(res) => match res {
                Ok((id, chunks)) => {
                    self.yt_id = id;
                    self.yt_chunks = chunks;
                    self.yt_pos = 0;
                    return self.update(Msg::NextChunk);
                }
                Err(e) => {
                    self.busy = false;
                    self.error = Some(e);
                }
            },
            Msg::NextChunk => {
                let Some(chunk) = self.yt_chunks.get(self.yt_pos).map(|c| c.text.clone()) else {
                    self.busy = false;
                    return Task::none();
                };
                self.busy = true;
                self.error = None;
                self.yt_pos += 1;
                return Task::perform(off_thread(move || claude::lines_from_transcript(&chunk)), Msg::Generated);
            }
            Msg::OpenVideo => {
                if let Some(c) = self.current_chunk() {
                    if let Err(e) = youtube::open_in_browser(&youtube::watch_url(&self.yt_id, c.start)) {
                        self.error = Some(e);
                    }
                }
                return operation::focus(INPUT_ID);
            }
            Msg::Home => {
                self.screen = Screen::Setup;
                self.error = None;
            }
        }
        Task::none()
    }

    fn finish_line(&mut self) {
        let target = &self.lines[self.idx].en;
        self.total_secs += self.line_start.map_or(0.0, |t| t.elapsed().as_secs_f64());
        self.total_correct += typing::count_correct(target, &self.typed);
        self.total_chars += target.chars().count();
        for w in typing::missed_words(target, &self.typed) {
            *self.stats.missed.entry(w).or_default() += 1;
        }
        self.stats.lines_typed += 1;
        self.typed.clear();
        self.line_start = None;
        self.idx += 1;
        if self.idx == self.lines.len() {
            self.stats.best_wpm = self.stats.best_wpm.max(self.wpm());
            self.screen = Screen::Writing;
        }
        self.stats.save();
    }

    /// 지금 치고 있는 유튜브 세트 (yt_pos는 다음 세트를 가리킨다).
    fn current_chunk(&self) -> Option<&youtube::Chunk> {
        self.yt_pos.checked_sub(1).and_then(|i| self.yt_chunks.get(i))
    }

    fn wpm(&self) -> f64 {
        typing::wpm(self.total_correct, self.total_secs)
    }

    fn accuracy(&self) -> f64 {
        if self.total_chars == 0 {
            100.0
        } else {
            self.total_correct as f64 * 100.0 / self.total_chars as f64
        }
    }

    // ---- 화면 ----

    fn view(&self) -> Element<'_, Msg> {
        let body = match self.screen {
            Screen::Setup => self.setup_view(),
            Screen::Typing => self.typing_view(),
            Screen::Writing => self.writing_view(),
        };
        let mut page = column![body].spacing(16);
        if let Some(e) = &self.error {
            page = page.push(card(text(e).color(RED)));
        }
        scrollable(container(page.max_width(820)).center_x(Fill).padding(32)).into()
    }

    fn setup_view(&self) -> Element<'_, Msg> {
        let levels = row(LEVELS.iter().map(|l| toggle(l, self.level == *l, Msg::Level(l)))).spacing(6);
        let genre = row![
            toggle("채팅", self.chat, Msg::Chat(true)),
            toggle("소설", !self.chat, Msg::Chat(false)),
        ]
        .spacing(6);
        let topic = text_input("예: ordering coffee, a rainy day…", &self.topic)
            .on_input(Msg::Topic)
            .on_submit_maybe((!self.busy).then_some(Msg::Generate))
            .padding(10);
        let go = button(text(if self.busy { "Claude가 문장을 쓰는 중…" } else { "문장 만들기" }).size(16))
            .on_press_maybe((!self.busy).then_some(Msg::Generate))
            .padding([10, 24]);

        let form = column![
            labeled("레벨", levels.into()),
            labeled("장르", genre.into()),
            labeled("주제", topic.into()),
            go,
        ]
        .spacing(14);

        let mut missed: Vec<_> = self.stats.missed.iter().collect();
        missed.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let missed_line = if missed.is_empty() {
            "아직 없음".to_string()
        } else {
            missed.iter().take(15).map(|(w, n)| format!("{w} ×{n}")).collect::<Vec<_>>().join("   ")
        };
        let record = column![
            text(format!("누적 {}줄 · 최고 {:.0} WPM", self.stats.lines_typed, self.stats.best_wpm)),
            text("자주 틀린 단어").size(13).color(MUTED),
            text(missed_line).font(Font::MONOSPACE).size(14),
        ]
        .spacing(6);

        let yt = column![
            text("유튜브로 연습").size(13).color(MUTED),
            row![
                text_input("https://www.youtube.com/watch?v=…", &self.yt_url)
                    .on_input(Msg::YtUrl)
                    .on_submit_maybe((!self.busy).then_some(Msg::YtLoad))
                    .padding(10),
                button(text(if self.busy { "가져오는 중…" } else { "자막 가져오기" }))
                    .on_press_maybe((!self.busy && !self.yt_url.trim().is_empty()).then_some(Msg::YtLoad))
                    .padding([10, 18]),
            ]
            .spacing(8),
            text("영어 자막을 10줄씩 따라 치고 영작합니다 (yt-dlp 필요)").size(13).color(MUTED),
        ]
        .spacing(8);

        column![
            text("typingEng").size(34).color(ACCENT),
            text("쉬운 영어를 따라 치고, 직접 써보세요").color(MUTED),
            card(form),
            card(yt),
            card(record),
        ]
        .spacing(16)
        .into()
    }

    fn typing_view(&self) -> Element<'_, Msg> {
        let line = &self.lines[self.idx];
        let target = &line.en;

        let done = Column::with_children(
            self.lines[..self.idx]
                .iter()
                .map(|l| text(format!("{}{}", l.who(), l.en)).font(Font::MONOSPACE).size(15).color(MUTED).into()),
        )
        .spacing(4);

        let spans: Vec<_> = target
            .chars()
            .zip(typing::char_states(target, &self.typed))
            .map(|(c, s)| {
                let (ch, color) = match s {
                    CharState::Correct => (c, GREEN),
                    CharState::Wrong if c == ' ' => ('_', RED),
                    CharState::Wrong => (c, RED),
                    CharState::Pending => (c, MUTED),
                };
                span::<(), Font>(ch.to_string()).color(color)
            })
            .collect();

        let input = text_input("", &self.typed)
            .id(INPUT_ID)
            .on_input(Msg::Typed)
            .on_submit(Msg::Submit)
            .font(Font::MONOSPACE)
            .size(24)
            .padding(12);

        let live_secs = self.total_secs + self.line_start.map_or(0.0, |t| t.elapsed().as_secs_f64());
        let live_correct = self.total_correct + typing::count_correct(target, &self.typed);
        let stats = row![
            text(format!("{:.0} WPM", typing::wpm(live_correct, live_secs))).color(ACCENT),
            text(format!("정확도 {:.0}%", self.accuracy())).color(ACCENT),
            text("줄 끝까지 친 뒤 Enter").size(13).color(MUTED),
        ]
        .spacing(20)
        .align_y(Center);

        column![
            row![
                text(format!("{} / {}", self.idx + 1, self.lines.len())).color(MUTED),
                iced::widget::space::horizontal(),
                self.current_chunk().map(|c| {
                    button(text(format!("▶ 영상 {}:{:02}부터", c.start / 60, c.start % 60)).size(13))
                        .style(button::text)
                        .on_press(Msg::OpenVideo)
                }),
                button(text("그만하기").size(13)).style(button::text).on_press(Msg::Home),
            ]
            .align_y(Center),
            done,
            card(
                column![
                    row![
                        text(line.who()).font(Font::MONOSPACE).size(24).color(ACCENT),
                        rich_text(spans).font(Font::MONOSPACE).size(24),
                    ],
                    text(&line.ko).size(15).color(MUTED),
                    input,
                ]
                .spacing(12)
            ),
            stats,
        ]
        .spacing(16)
        .into()
    }

    fn writing_view(&self) -> Element<'_, Msg> {
        let summary = text(format!("따라치기 완료 — {:.0} WPM · 정확도 {:.0}%", self.wpm(), self.accuracy()))
            .color(GREEN);

        let prompts = Column::with_children(
            self.lines.iter().enumerate().map(|(i, l)| text(format!("{}. {}{}", i + 1, l.who(), l.ko)).size(15).into()),
        )
        .spacing(4);

        let editor = text_editor(&self.editor)
            .placeholder("영어로 써보세요…")
            .on_action(Msg::Edit)
            .font(Font::MONOSPACE)
            .height(200)
            .padding(12);

        let can = !self.busy && !self.editor.text().trim().is_empty();
        let mut actions = row![
            button(text(if self.busy { "처리 중…" } else { "첨삭 받기" }))
                .on_press_maybe(can.then_some(Msg::AskReview))
                .padding([10, 24]),
        ]
        .spacing(10);
        if self.yt_pos < self.yt_chunks.len() {
            actions = actions.push(
                button(text(format!("다음 10줄 ({}/{})", self.yt_pos + 1, self.yt_chunks.len())))
                    .on_press_maybe((!self.busy).then_some(Msg::NextChunk))
                    .padding([10, 24]),
            );
        }
        actions = actions.push(button(text("새 문장")).style(button::secondary).on_press(Msg::Home).padding([10, 24]));

        let mut page = column![
            summary,
            card(
                column![
                    text("이제 직접 써보세요").size(22),
                    text("한국어를 보고 방금 친 영어를 떠올려 써보세요. 똑같지 않아도 됩니다.").size(13).color(MUTED),
                    prompts,
                    editor,
                    actions,
                ]
                .spacing(12)
            ),
        ]
        .spacing(16);

        if let Some(r) = &self.review {
            let fixes = Column::with_children(r.errors.iter().map(|f| {
                column![
                    rich_text![
                        span::<(), Font>(&f.original).color(RED).strikethrough(true),
                        span("  →  ").color(MUTED),
                        span(&f.fix).color(GREEN),
                    ],
                    text(&f.why_ko).size(13).color(MUTED),
                ]
                .spacing(2)
                .into()
            }))
            .spacing(10);
            let model = Column::with_children(
                self.lines
                    .iter()
                    .map(|l| text(format!("{}{}", l.who(), l.en)).font(Font::MONOSPACE).size(14).color(MUTED).into()),
            )
            .spacing(2);
            page = page.push(card(
                column![
                    text("교정문").size(13).color(MUTED),
                    text(&r.corrected).color(GREEN),
                    rule::horizontal(1),
                    fixes,
                    rule::horizontal(1),
                    text(&r.feedback_ko),
                    rule::horizontal(1),
                    text("모범 답안 (원문)").size(13).color(MUTED),
                    model,
                ]
                .spacing(12),
            ));
        }
        page.into()
    }
}

// ---- 작은 UI 조각 ----

fn card<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content).padding(20).width(Fill).style(container::rounded_box).into()
}

fn toggle(label: &str, on: bool, msg: Msg) -> Element<'_, Msg> {
    button(text(label))
        .style(if on { button::primary } else { button::secondary })
        .on_press(msg)
        .padding([6, 16])
        .into()
}

fn labeled<'a>(label: &'a str, content: Element<'a, Msg>) -> Element<'a, Msg> {
    row![text(label).width(48).color(MUTED), content].spacing(12).align_y(Center).into()
}

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title("typingEng")
        .theme(Theme::TokyoNight)
        .window_size((900.0, 720.0))
        .run()
}
