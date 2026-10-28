<p align="center">
  <img src="images/icon.png" width="128" height="128" alt="qswitch icon">
</p>

<h1 align="center">qswitch</h1>

<p align="center">Fast keyboard layout switcher: double Shift converts what you have just typed into the other layout</p>

<p align="center">
  <a href="https://github.com/Fan4Metal/qswitch/releases/latest"><img src="https://img.shields.io/github/v/release/Fan4Metal/qswitch?label=release" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/Windows-10%2B-0078D6" alt="Windows 10 or later">
  <img src="https://img.shields.io/badge/Rust-B7410E?logo=rust&logoColor=white" alt="Rust">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center"><b>English</b> | <a href="README.ru.md">Русский</a></p>

qswitch is a small keyboard layout switcher for Windows for those who write in two languages, in the spirit of Punto Switcher and Caramba Switcher. A word typed in the wrong layout (`ghbdtn` instead of `привет`, `зкштеа` instead of `printf`) is fixed with two taps of Shift: the word is replaced with the same keys read in the other layout, and the window is switched to that layout. The program lives in the notification area, shows the current layout there, and follows rules per program. Nothing typed is written to disk and nothing is sent anywhere.

![qswitch's Status page: the state, the layout and the program in front, a field to try the gesture in](images/screenshot.png)

## Features

- **Two taps of Shift**: the last word is converted into the other layout and the layout is switched (see [The gesture](#the-gesture)). The same gesture repeated puts the word back, and the layout too.
- **Selected text**: with nothing typed, the gesture converts the selection instead, in any program that exposes its text to UI Automation (editors, browsers, Office, messengers).
- **Automatic switching**, optional: a word typed in the wrong layout is converted by itself at the space after it; the letter statistics of the two languages decide, and a wrong guess is undone with the gesture (see [Automatic switching](#automatic-switching)).
- **Caps Lock as the layout switch**: optional; Shift+Caps Lock stays the real Caps Lock.
- **The layout in the notification area**: two letters (`EN`, `RU`) in the colour of the taskbar, on a blue square while the automatic switch is on, on a red one while the key trace or the list of conversions is on, grey while paused.
- **A badge near the caret**: the layout's letters appear under the caret for a moment whenever the layout changes, or stay beside it as it moves, in a size and a colour of choice (see [The badge near the caret](#the-badge-near-the-caret)).
- **Rules per program**: do nothing in games and remote desktops, start a program in a given layout (see [Programs](#programs)).
- **Any pair of installed layouts**, not only English and Russian.
- **Light, dark or system theme.**
- **English or Russian interface**: it follows the language of Windows and can be changed in the About window (`F1`).
- **Portable**: the [portable archive](#installation) keeps its settings next to the program.
- **Privacy**: nothing typed is kept on disk; no network access beyond the optional update check (see [Update check and privacy](#update-check-and-privacy)).

## Installation

The installer and the portable archive are on the [Releases](https://github.com/Fan4Metal/qswitch/releases) page. The executables are not code-signed, so Windows SmartScreen may warn about an unknown publisher on first launch.

- **Installer**, `qswitch_<version>_Setup.exe`: needs no administrator rights; the program is placed in `%LOCALAPPDATA%\Programs\qswitch` and keeps its settings in `%APPDATA%\qswitch`. It offers to start the program with Windows (the same setting is on the Settings page).
- **Portable archive**, `qswitch_<version>_portable.zip`: the program in a `qswitch` folder, run without installation; the settings are kept next to it, so it can be carried on a removable drive.

<details>
<summary>Where the portable program keeps its settings</summary>

The archive holds an empty `qswitch.json`. While that file is next to `qswitch.exe` and the folder is writable, the settings are kept in it. Without the file, the program keeps them in `%APPDATA%\qswitch`, as the installed program does. The About window shows where the settings are kept.

</details>

## The gesture

The program watches the keys typed since the caret last moved (a click, an arrow key, Enter, a change of window) and knows what each of them put on the screen in either layout of the pair. Two taps of Shift in a row, with nothing else pressed between them, do the following:

| Press | Effect |
|---|---|
| The first | The last word is erased and typed again as the same keys read in the other layout; the window is switched to that layout. |
| The second | The word is put back as it was typed, and the layout too. Each further repeat flips it again. |

The word is what was typed since the last space. A sign that is a sign in both layouts (a slash, a bracket, a colon, an at sign) placed before the letters ends the word, since it was meant as typed: `word/ghbdtn` becomes `word/привет`, and the part before the slash stays. Signs after the letters were typed blind with the word and are converted with it: `ghbdtn/` becomes `привет.`. A hyphen and an underscore join the parts of a word (`rnj-yb,elm` becomes `кто-нибудь`), and so does a key that is a letter in one of the layouts (`'nj` becomes `это`).

The Status page has a field to try the gesture in: a word typed there in the wrong layout is converted like anywhere else. Instead of two taps of Shift, the Settings page offers two taps of Ctrl or the Pause/Break key (as in Punto Switcher).

<details>
<summary>Selected text, and the gesture with nothing typed</summary>

With nothing typed yet, the gesture converts the selected text, if there is one: the selection is read through UI Automation, converted character by character (each goes to the other layout of the pair) and typed over the selection, and the window is switched to the layout of the result. With no selection either, the gesture only switches the layout. Programs that do not expose their text (and terminals, where typing does not replace a selection) get the layout switch alone. The editor of VS Code exposes its text only in its screen reader mode: with `"editor.accessibilitySupport": "on"` in its settings (or Shift+Alt+F1) the selection is converted there too.

</details>

<details>
<summary>How the conversion is typed, and where it does nothing</summary>

The conversion is done by erasing characters before the caret with Backspace and typing the new ones as Unicode, so it works in any program that takes keyboard input: editors, browsers, messengers, terminals. Programs that handle keys themselves (games with DirectInput, remote desktops, virtual machines) are better left alone with a rule (see [Programs](#programs)); a remote desktop, VirtualBox, VMware and Hyper-V are excluded from the start. Windows of programs running as administrator do not accept input from a program that is not, so the gesture does nothing there unless qswitch itself runs as administrator.

</details>

## Automatic switching

Off until turned on (the **Auto** button in the toolbar, or the Settings page), the automatic switch converts a word typed in the wrong layout at the space after it, as the gesture would, and switches the layout: `ghbdtn ` becomes `привет `, `руддщ ` becomes `hello `. A wrong guess is undone with the gesture, and that word is then left alone until the program restarts. The **Confidence** setting trades the words caught for the rare wrong guess: cautious, normal (the default) or bold. It works for a pair of English and Russian layouts.

<details>
<summary>How it decides, and what it leaves alone</summary>

Each language has a table of the cost of every three-letter combination, built from a word frequency list (`tools/make_ngrams.py`) and embedded in the program (60 KB for both). A word is read in the layout it was typed in and in the other one; when the reading in the other layout costs much less and is a plausible word by itself, the word is converted. On words the tables have not seen, the bold setting catches 95 % of the words typed in the wrong layout and misfires on 0.04 % of the words typed right; the cautious one catches about 70 % and misfires on none.

Nothing is converted in password fields (a classic password edit, or an element UI Automation reports as a password) and in terminals, where a password typed shows nothing. Words shorter than three letters, words with digits or with the signs of a path, an address or code (`/ \ @ : # & = % + _ ~ ^ | < > ( ) [ ] { } * $`), and words already flipped by the gesture are left alone, and so are the words put back with the gesture (kept in memory only). The word before Enter is not checked: in a chat, Enter sends it.

</details>

## The badge near the caret

When the layout changes, by the gesture or by Alt+Shift, a small badge with the layout's letters appears under the caret of the window in front and goes after 0.7 s, so the layout is seen where the eyes are, not in the corner of the screen. The Settings page offers three modes: off, for a moment when the layout changes (the default, with the time it stays), or always, following the caret. The badge takes no focus and lets clicks through.

<details>
<summary>Size, colours and the preview</summary>

The size of the badge's letters is set on the Settings page (10 pt by default), and the badge may take a colour of its own for each layout of the pair (a blue and a red until chosen) instead of the grey of the taskbar, so the layout is told by the colour before the letters are read. The badge appears beside the pointer while any of these is being changed, and on the **Show** button, so the look can be judged at once.

</details>

<details>
<summary>The caret placed, and how the caret is found</summary>

In the "for a moment" mode the badge also appears when the caret is placed: a click into text, another field, another window (a check box, on by default); a click on anything that takes no text shows nothing. In the "always" mode it follows the caret the same way through clicks and changes of window, and leaves when the focus goes to something that takes no text.

Its place comes from the caret the program in front reports to Windows (Notepad, Office and most classic programs); for programs that draw their own caret (Chrome, Edge, VS Code and other Electron applications, WPF and Store applications) it is asked through accessibility (UI Automation), which takes a few milliseconds. Only when neither answers does the badge appear beside the mouse pointer.

</details>

## Programs

The Programs page lists rules by the file name of a program's executable:

| Behaviour | Meaning |
|---|---|
| Do nothing | The gesture is off and nothing is tracked while a window of the program is in front. |
| Start in EN / Start in RU | When a window of the program comes to the front, it is put into that layout; the gesture works as usual. |

The list under the rules offers the programs that have a window on the screen now, the one that was in front before the qswitch window was opened first, so a rule for a game or a terminal is made without typing the name of its exe.

## Pause and the notification area

A click on the icon only switches the layout of the window that was in front, and the focus stays where it is; qswitch's own window opens from the icon's menu, which has **Show**, **Pause** (every key goes through untouched, the icon turns grey), **Automatic switching** (a check mark while it is on) and **Exit**. The window's close button and the Esc key hide it to the notification area (a setting on the Settings page); the program is ended from the icon's menu.

<details>
<summary>The command line</summary>

A second start of `qswitch.exe` only brings the window of the running copy forward. `qswitch.exe --pause`, `--resume` and `--quit` tell the running copy what to do (for scripts and game launchers); `--hidden` starts it without the window (the autostart entry); `--version` and `--help` print to the console.

</details>

## Keyboard shortcuts

| Key | Action |
|---|---|
| `Shift` `Shift` | Convert the last word, or put it back; with nothing typed, the selected text (the default gesture, see above) |
| `Esc` | Hide the window to the notification area |
| `F1` | About |

## Update check and privacy

The update check is optional and off by default. With **Check for updates at start-up (once a day)** enabled in the About window, the program asks the GitHub Releases API (`api.github.com`) for the latest release at most once a day; **Check now** in the same window asks at once. The request carries nothing but the program version in its `User-Agent` header, and nothing is downloaded or installed: when a newer release exists, a button with its version appears beside the About button and opens the release page in the browser. Otherwise the program does not access the network.

The keys typed are kept in memory only as long as they can still be converted (until the caret moves), and are never written to disk; the Status page's list of recent conversions is off until its check box is turned on, lives in memory while it is on and goes with the program. The one exception is the check box on the Status page that writes a trace of the keys to a file next to the settings, for finding out what a program does with the converted text; it is off at every start. While either is on, the toolbar says so in red and the icon in the notification area turns red, since passwords are typed and converted like any other word.

## Building

Requirements: Windows 10 or later, a stable Rust toolchain (MSVC target, 2024 edition) and the Visual Studio 2022 Build Tools with the C++ workload. For `build.py`, [uv](https://docs.astral.sh/uv/).

```
uv run build.py          # target\release\qswitch.exe
cargo test
```

<details>
<summary>How the build works</summary>

`build.py` first closes a `qswitch.exe` running from the project's `target` folder, because it would lock the file, then runs `cargo build --release`. When the program is not running, plain `cargo build --release` works as well. The executable links the C runtime statically (`.cargo/config.toml`), so it needs nothing beyond Windows itself.

</details>

<details>
<summary>Setting up the environment</summary>

The tools can be installed with winget:

```
winget install Rustlang.Rustup Microsoft.VisualStudio.2022.BuildTools Python.Python.3.12 astral-sh.uv JRSoftware.InnoSetup
```

The C++ workload of the Build Tools ("Desktop development with C++") is selected in the Visual Studio Installer.

</details>

## Installer and release

The release script builds the executable, the installer and the portable archive. It needs [Inno Setup 6](https://jrsoftware.org/isinfo.php).

```
python tools/make_release.py              # tests, release build, installer, archive
python tools/make_release.py --no-tests   # the same without cargo test
python tools/make_release.py --install    # then a silent installation over the installed copy
```

The script writes the files to `dist` and takes the version from `Cargo.toml`. The installer script is `tools/setup.iss`; `qswitch.exe --export-icon <file.ico>` writes the installer icon.

<details>
<summary>Releases on GitHub</summary>

Releases on GitHub are built by the **Release** workflow (`.github/workflows/release.yml`). After the version in `Cargo.toml` is updated and committed, pushing a matching tag publishes a release with the installer and the portable archive:

```
git tag v0.1.0
git push origin v0.1.0
```

The workflow stops if the tag does not match the version in `Cargo.toml`. Running the workflow manually (Actions → Release → Run workflow) builds the files and attaches them to the run, without making a release.

</details>

## Built with

qswitch is built on the following open-source projects:

- [egui / eframe](https://github.com/emilk/egui): the user interface and the window;
- [serde](https://serde.rs/) and [serde_json](https://github.com/serde-rs/json): the settings file;
- [FrequencyWords](https://github.com/hermitdave/FrequencyWords) (word frequency lists from OpenSubtitles, CC BY-SA 4.0): the letter statistics of the automatic switch are built from them;
- [Inno Setup](https://jrsoftware.org/isinfo.php): the installer.

## License

MIT, see [LICENSE](LICENSE).
