use crate::ui::widget::CommandLine;
use crate::{
    config::{AppMode, Command, ScreenEnum},
    ui::{screen::*, Controller},
    NCM_API, PLAYER,
};
use anyhow::Result;
use crossterm::{
    event,
    event::{Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge};
use std::collections::VecDeque;
use std::io::Stdout;

pub struct App<'a> {
    // model
    current_screen: ScreenEnum,
    current_mode: AppMode,
    need_re_update_view: bool,
    command_queue: VecDeque<Command>,

    // view
    main_screen: MainScreen<'a>,
    // playlist_screen: PlaylistScreen<'a>,
    login_screen: LoginScreen<'a>,
    help_screen: HelpScreen<'a>,
    command_line: CommandLine<'a>,
    playback_bar: Gauge<'a>,

    // const
    terminal: Terminal<CrosstermBackend<Stdout>>,
    normal_style: Style,
}

/// public
impl<'a> App<'a> {
    pub fn new(terminal: Terminal<CrosstermBackend<Stdout>>) -> Self {
        let normal_style = Style::default();
        let playback_bar = Gauge::default()
            .block(Block::default().borders(Borders::ALL))
            .gauge_style(normal_style)
            .ratio(0.0)
            .label("--:--/--:--");

        Self {
            current_screen: ScreenEnum::Main,
            current_mode: AppMode::Normal,
            need_re_update_view: true,
            command_queue: VecDeque::new(),
            main_screen: MainScreen::new(&normal_style),
            login_screen: LoginScreen::new(&normal_style),
            help_screen: HelpScreen::new(&normal_style),
            command_line: CommandLine::new(&normal_style),
            playback_bar,
            terminal,
            normal_style,
        }
    }

    /// cookie 登录/二维码登录后均调用
    pub async fn init_after_login(&mut self) -> Result<()> {
        let ncm_api_guard = NCM_API.lock().await;

        self.main_screen = MainScreen::new(&self.normal_style); // &normal_style
                                                                // self.playlist_screen = PlaylistScreen::new(&normal_style);

        if let (Some(playlist_name), Some(playlist)) = ncm_api_guard.user_favorite_songlist() {
            self.main_screen
                .update_playlist_model(playlist_name, playlist);
        }

        self.switch_screen(ScreenEnum::Main).await;

        Ok(())
    }

    pub fn restore_terminal(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;

        Ok(())
    }
}

/// Controller
impl<'a> App<'a> {
    pub async fn update_model(&mut self) -> Result<()> {
        // screen
        self.need_re_update_view = match self.current_screen {
            ScreenEnum::Help => false,
            ScreenEnum::Login => self.update_login_model().await?,
            ScreenEnum::Main => self.main_screen.update_model().await?,
        };

        // playback_bar
        let player_guard = PLAYER.lock().await;
        if let Some(player_position) = player_guard.position() {
            if let Some(player_duration) = player_guard.duration() {
                self.playback_bar = self.playback_bar.clone().label(format!(
                    "{:02}:{:02}/{:02}:{:02}",
                    player_position.minutes(),
                    player_position.seconds() % 60,
                    player_duration.minutes(),
                    player_duration.seconds() % 60,
                ));
            }
        }

        Ok(())
    }

    pub async fn handle_event(&mut self) -> Result<bool> {
        // 解析命令
        if let Event::Key(key_event) = event::read()? {
            if key_event.kind == KeyEventKind::Press || key_event.kind == KeyEventKind::Repeat {
                match (&self.current_mode, key_event.code) {
                    // Normal 模式下按键
                    (AppMode::Normal, _) => self.get_command_from_key(key_event.code),

                    // CommandEntry 模式下
                    (AppMode::CommandEntry, KeyCode::Enter) => {
                        self.parse_command();
                        self.back_to_normal_mode();
                    }
                    (AppMode::CommandEntry, KeyCode::Esc) => {
                        self.command_line.reset();
                        self.back_to_normal_mode();
                    }
                    (AppMode::CommandEntry, _) => {
                        self.command_line.textarea.input(key_event);
                    }
                }
            }
        }

        // 执行命令
        if let Some(cmd) = self.command_queue.pop_front() {
            match cmd {
                Command::Quit => {
                    return Ok(false);
                }
                Command::GotoScreen(to_screen) => {
                    self.switch_screen(to_screen).await;
                }
                Command::EnterCommand => {
                    self.switch_to_command_entry_mode();
                    self.command_line.reset();
                    self.command_line.set_prompt(":");
                }
                Command::Logout => {
                    self.login_screen = LoginScreen::new(&self.normal_style);
                    // TODO: 清除 cache
                    NCM_API.lock().await.logout().await;
                }
                // 需要向下传递的事件
                Command::Down
                | Command::Up
                | Command::NextPanel
                | Command::PrevPanel
                | Command::Esc
                | Command::Play => {
                    // 先 update_model(), 再 handle_event()
                    // 取或值
                    self.need_re_update_view = self.need_re_update_view
                        || match self.current_screen {
                            ScreenEnum::Main => self.main_screen.handle_event(cmd).await?,
                            ScreenEnum::Login => self.login_screen.handle_event(cmd).await?,
                            ScreenEnum::Help => self.help_screen.handle_event(cmd).await?,
                        };
                }
                _ => {}
            }
        }

        Ok(true)
    }

    pub fn update_view(&mut self) {
        // screen 只在 need_re_update_view 为 true 时更新view
        if self.need_re_update_view {
            match self.current_screen {
                ScreenEnum::Help => {}
                ScreenEnum::Login => self.login_screen.update_view(&self.normal_style),
                ScreenEnum::Main => self.main_screen.update_view(&self.normal_style),
            }
        }

        // command_line
        let show_cursor = match self.current_mode {
            AppMode::Normal => false,
            AppMode::CommandEntry => true,
        };
        self.command_line.set_cursor_visibility(show_cursor);
        self.command_line.update_view(&self.normal_style);
    }

    pub fn draw(&mut self) -> Result<()> {
        //
        self.update_view();

        //
        self.terminal.draw(|frame| {
            //
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Min(3),
                        Constraint::Length(3),
                        Constraint::Length(1),
                    ]
                    .as_ref(),
                )
                .split(frame.area());

            // render screen
            match self.current_screen {
                ScreenEnum::Help => self.help_screen.draw(frame, chunks[0]),
                ScreenEnum::Login => self.login_screen.draw(frame, chunks[0]),
                ScreenEnum::Main => self.main_screen.draw(frame, chunks[0]),
            }

            // render info_widget & playback_bar
            let playback_chunk = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(26), Constraint::Min(3)].as_ref())
                .split(chunks[1]);
            frame.render_widget(&self.playback_bar, playback_chunk[1]);

            // render command_line
            self.command_line.draw(frame, chunks[2]);
        })?;

        Ok(())
    }
}

/// private
impl<'a> App<'a> {
    fn get_command_from_key(&mut self, key_code: KeyCode) {
        let cmd = match key_code {
            KeyCode::Char('k') => Command::Up,
            KeyCode::Up => Command::Up,
            KeyCode::Char('j') => Command::Down,
            KeyCode::Down => Command::Down,
            KeyCode::Char(' ') => Command::TogglePlay,
            KeyCode::Char(',') => Command::PrevTrack,
            KeyCode::Char('.') => Command::NextTrack,
            // KeyCode::Enter => Command::QueueAndPlay,
            KeyCode::Enter => Command::Play,
            KeyCode::Esc => Command::Esc,
            KeyCode::Char('r') => Command::ToggleRepeat,
            KeyCode::Char('s') => Command::ToggleShuffle,
            KeyCode::Char('g') => Command::GotoTop,
            KeyCode::Char('G') => Command::GotoBottom,
            KeyCode::Right => Command::NextPanel,
            KeyCode::Tab => Command::NextPanel,
            KeyCode::Left => Command::PrevPanel,
            KeyCode::BackTab => Command::PrevPanel,
            KeyCode::Char('1') => Command::GotoScreen(ScreenEnum::Main),
            // KeyCode::Char('2') => Command::GotoScreen(ScreenEnum::Playlists),
            KeyCode::Char('0') => Command::GotoScreen(ScreenEnum::Help),
            KeyCode::F(1) => Command::GotoScreen(ScreenEnum::Help),
            KeyCode::Char('n') => Command::NewPlaylist(None),
            KeyCode::Char('p') => Command::PlaylistAdd,
            KeyCode::Char('x') => Command::SelectPlaylist,
            KeyCode::Char('q') => Command::Quit,
            KeyCode::Char(':') => Command::EnterCommand,
            _ => Command::Nop,
        };

        self.command_queue.push_back(cmd);
    }

    fn parse_command(&mut self) {
        let input_cmd = self.command_line.get_contents();

        self.command_line.reset();

        match Command::parse(&input_cmd) {
            Ok(cmd) => {
                self.command_queue.push_back(cmd);
            }
            Err(e) => {
                self.show_prompt(format!("{e}").as_str());
            }
        }
    }

    fn back_to_normal_mode(&mut self) {
        self.current_mode = AppMode::Normal;
    }

    fn switch_to_command_entry_mode(&mut self) {
        self.current_mode = AppMode::CommandEntry;
    }

    fn show_prompt(&mut self, text: &str) {
        self.command_line.textarea.insert_str(text);
    }

    async fn update_login_model(&mut self) -> Result<bool> {
        //
        let need_redraw = self.login_screen.update_model().await?;

        if NCM_API.lock().await.is_login() {
            // 登录成功
            self.init_after_login().await?;
            Ok(true)
        } else {
            Ok(need_redraw)
        }
    }

    async fn switch_screen(&mut self, to_screen: ScreenEnum) {
        if to_screen == ScreenEnum::Login && NCM_API.lock().await.is_login() {
            self.show_prompt("you have to logout from current account first!");
            return;
        }

        self.need_re_update_view = true;
        self.current_screen = to_screen;
    }
}
