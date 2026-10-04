# Templates

A template is a finished edit with empty places, called slots. You choose
your clips. chukcut puts them into the slots and cuts them to the template's
music.

## The built-in templates

chukcut has 11 templates: **Quick Cuts**, **Travel Diary**, **Product
Spotlight**, **Vlog Intro**, **Cinematic Trailer**, **Memories**, **Before /
After**, **Photo Dump**, **Talking Points**, **Retro Tape** and
**Reaction**. They are made from chukcut's own parts: text styles, LUT
looks, animations and music.

You find them in two places:

- On the start screen, under **Templates**.
- In the asset panel's **Templates** tab: **All**, **Social**, **Travel &
  vlog**, **Business**, **Cinematic**, **My templates** and **This project**.

## Use a template

1. Click a template. The **Use template** dialog opens.
2. When a project is open, choose where the result goes:
   - **New project**
   - **New timeline**: a new tab in this project.
   - **Compound clip at playhead**: one compound clip on the timeline, at
     the playhead.

   From the start screen, the result is always a new project.
3. For each slot, click **Choose…** and pick a clip. **Choose several…**
   fills many slots at once, in order. **Change** picks another clip. The
   **X** clears a slot.
4. Click **Create project**, **Add timeline** or **Add compound clip**.

How your clips fit: a longer clip is trimmed, a shorter clip is slowed, and
a clip with another shape is cropped. Fill the slots in order. An empty slot
before a filled one would move the clips up.

You can start with empty slots. They show a placeholder. A message then
says how to fill them later.

A template made for another shape (for example 9:16 in a 16:9 project) is
laid out on the project's shape.

## Fill or change slots later

In the asset panel, open **Templates › This project**:

- **Slots (N of M filled)…** opens the **Template slots** dialog. Each slot
  has **Fill…** or **Replace…**.
- In the timeline, **Replace media…** in a clip's right-click menu also
  fills a slot.

Slots inside a compound clip stay slots.

## Save your own template

1. Make a project. Select the clips that are to become slots. With no
   selection, the slots that the project has stay slots.
2. **Templates › This project › Save as template…**
3. Enter a **Template name** and, if you want, **What it is for
   (optional)**.
4. Click **Save template**.

The template then shows under **My templates**. chukcut copies the media
that the template needs, so a project made from it still works when the
template is deleted. Your templates are in
`~/.local/share/chukcut/templates/`.

## From the command line

`chukcut-cli template list | apply | slots | replace | save | delete`. See
[`docs/cli.md`](../cli.md#templates).
