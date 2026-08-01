//! Terminal setup with reliable restoration on every return path.

use std::io::{self, Stdout};

use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    cursor::SetCursorStyle,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement},
};
use ratatui::{Terminal, backend::CrosstermBackend};

/// Owns raw mode and alternate-screen lifetime.
pub struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    keyboard_enhancement_enabled: bool,
    command_cursor_active: bool,
}

impl TerminalSession {
    /// Enter the alternate screen and start a ratatui terminal.
    pub fn start() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        // Conventional terminal input collapses Ctrl+Backspace into the same
        // byte sequence as other control keys.  Kitty's progressive keyboard
        // protocol preserves the modifier so crossterm can report it as an
        // actual Ctrl+Backspace event.
        let keyboard_enhancement_enabled = supports_keyboard_enhancement().unwrap_or(false)
            && execute!(
                stdout,
                // Keep escape/modifier sequences unambiguous without asking
                // the terminal to report physical keys for ordinary text.
                // The latter loses the layout-resolved character (for
                // example Shift+1), which text fields need verbatim.
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )
            .is_ok();
        if let Err(error) = execute!(stdout, EnableMouseCapture) {
            if keyboard_enhancement_enabled {
                let _ = execute!(stdout, PopKeyboardEnhancementFlags);
            }
            let _ = execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error);
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut stdout = io::stdout();
                let _ = execute!(stdout, DisableMouseCapture);
                if keyboard_enhancement_enabled {
                    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
                }
                let _ = execute!(stdout, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(error);
            }
        };
        Ok(Self { terminal, keyboard_enhancement_enabled, command_cursor_active: false })
    }

    /// Borrow the ratatui terminal for drawing.
    pub fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    /// Use a blinking bar only while the command popup owns text input.
    pub fn set_command_cursor_active(&mut self, active: bool) -> io::Result<()> {
        if self.command_cursor_active == active {
            return Ok(());
        }
        execute!(
            self.terminal.backend_mut(),
            if active {
                SetCursorStyle::BlinkingBar
            } else {
                SetCursorStyle::BlinkingBlock
            }
        )?;
        self.command_cursor_active = active;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture
        );
        if self.keyboard_enhancement_enabled {
            let _ = execute!(self.terminal.backend_mut(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = execute!(
            self.terminal.backend_mut(),
            crossterm::cursor::SetCursorStyle::BlinkingBlock
        );
        let _ = self.terminal.show_cursor();
    }
}
