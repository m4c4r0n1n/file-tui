# file-tui

A small rewrite of file-browser-tui by adelmonte at https://github.com/adelmonte/file-browser-tui with a few adjustments and tweaks to work with Ghostty and NixOS cleanly. 

<img src="https://github.com/user-attachments/assets/d115502b-3206-4dd4-8dd4-d1a3744b8684" width="100%" alt="file-tui showing the home folder" />

<table>
  <tr>
    <td width="50%"><img src="https://github.com/user-attachments/assets/24086848-6285-446b-930d-66f6a8393031" width="100%" alt="file-tui showing a Music folder with album art thumbnails" /></td>
    <td width="50%"><img src="https://github.com/user-attachments/assets/be30a066-0a15-4bdd-8e4b-12c6e849649f" width="100%" alt="file-tui showing a Screenshots folder" /></td>
  </tr>
</table>

## Keybinds

- `Ctrl+C`: Quit
- `Enter`: Open
- `Esc`: Clear the selection / close search
- `Left`: Move left (Details: collapse)
- `Right`: Move right (Details: expand)
- `Up`: Move up
- `Down`: Move down
- `Shift+Left`: Extend selection left
- `Shift+Right`: Extend selection right
- `Shift+Up`: Extend selection up
- `Shift+Down`: Extend selection down
- `Home`: First item
- `End`: Last item
- `PageUp`: Up a page
- `PageDown`: Down a page
- `Shift+Home`: Extend selection to the first item
- `Shift+End`: Extend selection to the last item
- `Shift+PageUp`: Extend selection up a page
- `Shift+PageDown`: Extend selection down a page
- `F1`: Show the keybinds
- `Ctrl+F`: Search
- `Ctrl+A`: Select all
- `Ctrl+L`, `F6`, `Alt+D`: Edit the location
- `Backspace`, `Alt+Left`: Back
- `Alt+Right`: Forward
- `Alt+Up`: Up a folder
- `F5`: Reload
- `Ctrl+H`, `Alt+.`: Show/hide hidden files
- `F10`: New folder
- `F2`: Rename
- `Ctrl+C`: Copy
- `Ctrl+X`: Cut
- `Ctrl+V`: Paste
- `Ctrl+Alt+C`: Copy location
- `Ctrl+Z`: Undo
- `Delete`: Move to trash
- `Shift+Delete`: Delete permanently
- `Alt+Enter`: Properties
- `Shift+F4`: Open terminal here
- `Ctrl+Plus`, `Ctrl+=`: Larger icons
- `Ctrl+Minus`: Smaller icons
- `Ctrl+1`: Icons view
- `Ctrl+2`: Compact view
- `Ctrl+3`: Details view
- `Alt+[`: Narrower sidebar
- `Alt+]`: Wider sidebar
- `Ctrl+T`: New tab
- `Ctrl+W`: Close tab
- `Ctrl+Tab`, `Ctrl+PageDown`: Next tab
- `Ctrl+Shift+Tab`, `Ctrl+PageUp`: Previous tab
- `F3`: Split view on/off
- `Ctrl+F3`: Switch to the other side of a split
- `Arrows`: Move selection (Shift extends)
- `Double-click`: Open
- `Right-click`: Context menu
- `Drag`: Move (Ctrl copies) / select

Inspired by [file-browser-tui](https://github.com/adelmonte/file-browser-tui).
