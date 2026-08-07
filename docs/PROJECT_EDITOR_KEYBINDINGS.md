# Projects editor keyboard reference

The Projects editor uses a small Vim-inspired Normal mode together with Insert
mode and linewise Visual mode. Open a project file, then focus the editor with
`Alt+2` if needed.

## Modes and cursor shapes

| Mode | Enter | Cursor | Leave |
| :--- | :--- | :--- | :--- |
| Normal | `Esc` from Insert mode | block | `i` enters Insert; `V` enters Visual Line |
| Insert | `i`, or paste from Normal mode | bar | `Esc` |
| Visual Line | `V` from Normal mode | block | `Esc` or `V` |
| Pending command | first `g` or `d` | underscore | complete, cancel, or wait one second |

`gg` and `dd` are pending commands. Press the same second key to complete the
command. Pressing `Esc`, waiting for one second, or using another key cancels
the pending command; another key is then handled normally.

## Normal mode

### Movement

| Key | Action |
| :--- | :--- |
| `h` / `←` | move one character left |
| `l` / `→` | move one character right |
| `j` / `↓` | move down one line |
| `k` / `↑` | move up one line |
| `w` / `b` | move to the next / previous word boundary |
| `0` / `Home` | first character of the current line |
| `$` / `End` | end of the current line |
| `gg` | first line of the file |
| `G` | first character of the last line |
| `{` / `}` | previous / next paragraph |
| `%` | matching `()`, `[]`, or `{}` delimiter under the cursor |
| `Page Up` / `Page Down` | move by one visible editor page |
| mouse wheel | scroll by visible editor rows and move the cursor with it |

### Editing and history

| Key | Action |
| :--- | :--- |
| `i` | enter Insert mode |
| `x` / `Delete` | delete the character under the cursor |
| `dd` | delete the current line, including its newline when present |
| `Ctrl+Backspace` | delete the previous word |
| `Ctrl+Delete` | delete the next word |
| `u` / `Ctrl+r` | undo / redo the most recent editor change |

`Backspace` in Normal mode moves left; it does not delete text.

### Selection, clipboard, and project actions

| Key | Action |
| :--- | :--- |
| `V` | start Visual Line mode for whole-line selection |
| `Ctrl+Shift+V` (or `Ctrl+V`) | paste clipboard text; from Normal mode this also enters Insert mode |
| `Ctrl+s` | save the open source file |
| `Ctrl+f` | open citation search and insert a selected citation |
| `Esc` | return to the project file tree |

On terminals that do not preserve the Shift modifier for this shortcut,
`Ctrl+V` is accepted as the same paste command. Pasting into `.tex` and `.bib`
files preserves the clipboard text exactly; `.bib` files are saved immediately.

## Insert mode

Printable keys insert text at the cursor. These editing keys are also available:

| Key | Action |
| :--- | :--- |
| `Esc` | return to Normal mode |
| `Enter` / `Tab` | insert a newline / tab |
| arrow keys | move the cursor; `Up` and `Down` follow wrapped display rows |
| `Backspace` / `Delete` | delete the previous / next character |
| `Ctrl+Backspace` / `Ctrl+Delete` | delete the previous / next word |
| `Page Up` / `Page Down` | move by one visible editor page |
| `Ctrl+Shift+V` (or `Ctrl+V`) | paste clipboard text |

When a citation completion menu is open, use `Up`/`Down` to choose an entry and
`Tab` or `Enter` to insert it. `Esc` closes the menu first.

## Visual Line mode

Visual Line mode selects complete source lines from the line where `V` was
pressed to the current cursor line. The following motions extend or shrink the
selection:

| Key | Action |
| :--- | :--- |
| `j` / `↓`, `k` / `↑` | extend selection down / up by a line |
| `gg` / `G` | extend selection to the first / last line of the file |
| `{` / `}` | extend selection to the previous / next paragraph |
| `%` | extend selection to the line containing the matching delimiter |
| `y` | copy the selected lines to the system clipboard and return to Normal mode |
| `d` | delete the selected lines and return to Normal mode |
| `Esc` / `V` | cancel the selection and return to Normal mode |

`h`, `l`, and the left/right arrows can still move the cursor, but linewise
selection highlighting changes only when the cursor reaches another line.
