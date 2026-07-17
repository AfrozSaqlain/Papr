//! Terminal setup with reliable restoration on every return path.

use std::io::{self, Stdout};

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

/// Owns raw mode and alternate-screen lifetime.
pub struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    mouse: bool,
}

impl TerminalSession {
    /// Enter the alternate screen and start a ratatui terminal.
    pub fn start(mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        if mouse {
            if let Err(error) = execute!(stdout, EnableMouseCapture) {
                let _ = execute!(stdout, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(error);
            }
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut stdout = io::stdout();
                if mouse {
                    let _ = execute!(stdout, DisableMouseCapture);
                }
                let _ = execute!(stdout, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(error);
            }
        };
        Ok(Self { terminal, mouse })
    }

    /// Borrow the ratatui terminal for drawing.
    pub fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.mouse {
            let _ = execute!(self.terminal.backend_mut(), DisableMouseCapture);
        }
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = execute!(self.terminal.backend_mut(), crossterm::cursor::SetCursorStyle::BlinkingBlock);
        let _ = self.terminal.show_cursor();
    }
}
