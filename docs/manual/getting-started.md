# Getting started

## Install

chukcut runs on Linux only. You need a Vulkan driver for your GPU.

1. Install the system packages. The list is in the
   [README](../../README.md#system-packages).
2. In a clone of the repository, run `scripts/install.sh`. The script builds
   the editor and installs it in `~/.local/bin`, with a menu entry.
3. For the AI tools, also build the worker program, and put it next to the
   editor:

   ```bash
   cargo build --release -p chukcut-ml-worker
   install -m755 target/release/chukcut-ml-worker ~/.local/bin/
   ```

   Without the worker, the editor works, but the AI tools say that they
   cannot run.

Start chukcut from the menu, or from a terminal:

```bash
chukcut                      # the start screen
chukcut clip1.mp4 clip2.mov  # a new project with these files
chukcut my.chukcut           # open a project
```

## The start screen

The start screen opens when you start chukcut without a file.

- The sidebar has **Home**, **Shortcuts** (the shortcuts sheet) and
  **Settings**.
- Under **Start creating**:
  - **New project** makes an empty project.
  - The canvas chips set the shape: **9:16** (1080×1920), **16:9**
    (1920×1080), **1:1** (1080×1080) and **4:5** (1080×1350).
  - The name field. An empty name gives "Untitled".
  - The frame rate list: 24, 25, 30, 50 or 60 fps.
  - **Open project…** opens a `.chukcut` file.
- **Templates** shows the built-in templates. Click one to make a project
  from it. See [Templates](templates.md).
- **Projects** shows your recent projects, with a picture, the time you last
  opened each one, and its length. A project whose file is gone has a red
  **Missing** badge. The **X** on a card removes it from the list. The file
  stays on the disk.

When you click a canvas or a frame rate, the project keeps that choice. When
you do not click one, the project takes the shape and the frame rate of the
first clip that you import. Settings › **New projects** sets the canvas and
the frame rate that the start screen selects first.

## Your first project

1. Click **New project**.
2. Import a video: **Menu › Import media…** or `Ctrl+I`. The file appears in
   the asset panel's **Media** tab.
3. Drag the file onto the timeline. Or point at the tile and click its
   **+**: chukcut puts the file at the end of the first lane of its kind.
4. Press `Space` to play. Press `S` to split the clip at the playhead.
5. Click a clip to select it. The inspector shows its settings.
6. Click **Export** at the top right (or `Ctrl+E`). See [Export](export.md).

## Save, autosave and recovery

- **Menu › Save** (`Ctrl+S`) writes the project to its `.chukcut` file.
  **Save as…** writes it to a new file.
- A project is one JSON file. The media stays where it is. The project
  refers to it by its path.
- The title bar shows **Saved**, or **Autosaved just now** /
  **Autosaved N min ago** when there are changes that are not in the file.
- chukcut writes a working copy after every edit. This copy is not the
  project file. If chukcut stops without a clean exit, the next start asks
  **Restore unsaved work?** with **Not now**, **Discard** and **Restore**.
  **Not now** keeps the offer: a banner on the start screen shows it again.
- Before **New project**, **Open**, **Quit** or closing the window with
  changes that are not saved, chukcut asks **Save changes to "name"?** with
  **Cancel**, **Don't save** and **Save**.

## The menu

The **Menu** button at the top left holds: **New project**, **Open…**,
**Save**, **Save as…**, **Import media…**, **Export…**, **Settings…**,
**Keyboard shortcuts** and **Quit**. The keys for each item are in
[Keyboard shortcuts](shortcuts.md).

## File dialogs

chukcut uses the desktop's file dialog (xdg-desktop-portal). When the desktop
has no portal, chukcut opens its own file browser. It has places (Home, your
XDG folders, Computer), recent folders, a path field (type `~` for your home
folder), **Show hidden files** and **Show all files**.
`CHUKCUT_FILE_DIALOG=builtin` always uses the built-in browser.
`CHUKCUT_FILE_DIALOG=portal` never uses it.

## Preparing a project

When you open a project, chukcut makes the frames that are not in the cache:
background mattes, slow-motion frames, remade frames and the sound of
compound clips. A chip in the title bar shows the progress, for example
**Preparing 209 frames · 43 %**, with **Stop**. You can edit while it runs.
The chip goes away when the work is done.
