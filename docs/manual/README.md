# The chukcut user manual

This manual tells you how to use chukcut. Each page explains one part of the
app. The pages describe the current code. Labels in **bold** are the words
that you see on the screen. A path such as **Video › Enhance** means: open
the **Video** tab, then the **Enhance** sub-tab.

## The screen

```
+--------------------------------------------------------------+
| Menu   Saved        project name  1080×1920 · 30 fps  Export |  title bar
+------------------+------------------------+------------------+
| asset panel      | player                 | inspector        |
| (Media, Audio,   |                        | (Details, or the |
|  Text, ...)      |                        |  selected clip)  |
+------------------+------------------------+------------------+
| timeline: toolbar, timeline tabs, lanes, clips               |
+--------------------------------------------------------------+
```

- The **asset panel** (top left) holds your media and the libraries: text
  styles, stickers, effects, transitions, filters, captions, stock and
  templates.
- The **player** (top centre) shows the frame at the playhead.
  **Full screen** (the button at the bottom right of the player, or
  `Ctrl+Shift+F`) shows only the player on the whole screen. `Esc` or the
  same button goes back to the editor. A zoomed or cropped clip is decoded
  at the size it is drawn, so it stays sharp in the player.
- The **inspector** (top right) shows the settings of the selected clip.
  When no clip is selected, it shows the project's **Details**.
- The **timeline** (bottom) holds the clips in lanes.

## Pages

1. [Getting started](getting-started.md): install, the start screen, your first project
2. [Media and import](media.md): import, the media list, missing media, stock
3. [Timeline editing](timeline.md): select, cut, trim, move, markers, lanes
4. [Timelines and compound clips](compound-clips.md): several timelines, clips inside a clip
5. [Templates](templates.md): make a project from a template, save your own
6. [Text and captions](text-and-captions.md): titles, styles, auto captions, SRT
7. [Colour](colour.md): adjust, HSL, curves, wheels, LUTs, auto adjust, colour match
8. [Effects and transitions](effects-and-transitions.md): effects, effect clips, crop, masks, chroma key, stickers, noise reduction
9. [Speed and slow motion](speed.md): speed, curves, speed effects, frame blending, optical flow
10. [Tracking](tracking.md): make a title follow an object or a face
11. [AI tools](ai-tools.md): each AI tool, its model, its cost on GPU and CPU, its cache
12. [Audio](audio.md): volume, fades, cleanup, effects, isolate voice, silences, voiceover
13. [Export](export.md): presets, codecs, hardware encoders, the queue
14. [Keyboard shortcuts](shortcuts.md): every action and its keys, in each preset
15. [CLI and MCP](cli-and-mcp.md): edit projects from a shell or an AI agent
16. [Settings](settings.md): every setting and what it does
17. [Troubleshooting](troubleshooting.md): hardware decode, CUDA bundles, cache size, where files live
