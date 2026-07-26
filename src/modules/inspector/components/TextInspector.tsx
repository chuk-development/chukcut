/**
 * Everything about a title that is not its position on the timeline.
 *
 * ## Why there is a draft
 *
 * The document is server state: every edit replaces it wholesale. A textarea
 * bound straight to it would show the user their own keystroke only after a
 * round trip through Rust, which at the speed people type is a cursor that
 * jumps. So this panel keeps a draft, renders from that, and pushes to Rust on
 * a timer.
 *
 * ## Why it is debounced, and by how much
 *
 * A keystroke costs a rasterisation and a new preview session. The
 * rasterisation is 6–30 ms cold (`docs/research/text-rendering.md`) and the
 * session restart is what actually shows it. Neither is free at forty
 * characters a minute, and neither is *slow* enough to justify making the user
 * press a button. {@link COMMIT_DEBOUNCE_MS} is the pause after which a burst
 * of edits becomes one write; the preview then adds its own 160 ms restart
 * debounce on top, so the whole cost of typing a sentence is one re-render a
 * short moment after the typing stops.
 *
 * Anything that is not typing — a switch, a colour, an alignment button —
 * still goes through the same timer, which costs an imperceptible pause and
 * means there is exactly one path into the document from this panel.
 */

import {
  AlignCenterIcon,
  AlignLeftIcon,
  AlignRightIcon,
  BoldIcon,
  ItalicIcon,
  TypeIcon,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { runEdit } from "@/modules/project/store";
import type {
  Project,
  Rgba,
  Segment,
  TextAlign,
  TextMaterial,
  Transform,
} from "@/modules/project/types";
import { textSet } from "@/modules/text/lib/api";
import { hexToRgba, rgbaToCss, rgbaToHex, withAlpha } from "@/modules/text/lib/color";
import {
  DEFAULT_BACKGROUND,
  defaultOutlineWidth,
  defaultShadow,
} from "@/modules/text/lib/material";
import { useTextStore } from "@/modules/text/store";
import { timelineApply } from "@/modules/timeline/lib/api";

/** How long a burst of edits is folded into one write. See the module comment. */
export const COMMIT_DEBOUNCE_MS = 140;

// ---------------------------------------------------------------------------
// The draft
// ---------------------------------------------------------------------------

/**
 * A local copy of the title that the panel renders from, and a debounced writer.
 *
 * The document is adopted again whenever it says something other than what was
 * last sent — a different title selected, an undo, an edit from anywhere else.
 * Comparing against *what was sent* rather than against the previous document
 * is what stops the panel fighting its own echo: the response to a write is a
 * new document object with identical contents, and adopting that would reset
 * the cursor on every keystroke.
 */
function useTitleDraft(material: TextMaterial) {
  const [draft, setDraft] = useState(material);
  const draftRef = useRef(material);
  const sentRef = useRef(JSON.stringify(material));
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const incoming = JSON.stringify(material);
    if (incoming === sentRef.current) return;
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    sentRef.current = incoming;
    draftRef.current = material;
    setDraft(material);
  }, [material]);

  // A panel that unmounts mid-burst — the clip was deselected, or deleted —
  // must not leave a write for a title that may no longer be there.
  useEffect(
    () => () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    },
    [],
  );

  const edit = useCallback((patch: Partial<TextMaterial>) => {
    const next = { ...draftRef.current, ...patch };
    draftRef.current = next;
    setDraft(next);

    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => {
      timerRef.current = null;
      const sending = draftRef.current;
      sentRef.current = JSON.stringify(sending);
      void runEdit(() => textSet(sending));
    }, COMMIT_DEBOUNCE_MS);
  }, []);

  return { draft, edit };
}

// ---------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[76px_1fr] items-center gap-2">
      <span className="truncate text-[11px] text-muted-foreground">{label}</span>
      <div className="flex min-w-0 items-center gap-1.5">{children}</div>
    </div>
  );
}

/**
 * A colour swatch and, when the colour is allowed to be transparent, its alpha.
 *
 * `<input type="color">` has no alpha channel anywhere, so the two are separate
 * controls over one value — which is also how the document stores it.
 */
function ColorField({
  label,
  value,
  onChange,
  withOpacity = false,
}: {
  label: string;
  value: Rgba;
  onChange: (next: Rgba) => void;
  withOpacity?: boolean;
}) {
  return (
    <Row label={label}>
      <input
        type="color"
        aria-label={label}
        value={rgbaToHex(value)}
        onChange={(event) => onChange(hexToRgba(event.target.value, value[3]))}
        className="h-[22px] w-[34px] shrink-0 cursor-pointer rounded-sm border border-input bg-background p-0.5"
      />
      <span
        aria-hidden="true"
        className="size-[18px] shrink-0 rounded-sm border border-border"
        style={{ background: rgbaToCss(value) }}
      />
      {withOpacity ? (
        <input
          type="range"
          aria-label={`${label} opacity`}
          min={0}
          max={100}
          step={1}
          value={Math.round(value[3] * 100)}
          onChange={(event) => onChange(withAlpha(value, Number(event.target.value) / 100))}
          className="min-w-0 flex-1 accent-primary"
        />
      ) : null}
    </Row>
  );
}

function NumberField({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (next: number) => void;
}) {
  return (
    <Row label={label}>
      <Input
        type="number"
        aria-label={label}
        value={Number.isFinite(value) ? value : 0}
        min={min}
        max={max}
        step={step}
        onChange={(event) => {
          // A cleared field, or one holding "-" mid-typing, is not a value.
          // `Number("")` is 0, so committing it would silently snap the size to
          // the minimum the moment somebody selects-all and starts retyping —
          // and a NaN from anywhere else is a project that never opens again,
          // because `serde_json` writes it as `null`.
          const raw = event.target.value.trim();
          if (raw === "") return;
          const next = Number(raw);
          if (Number.isFinite(next)) onChange(Math.min(max, Math.max(min, next)));
        }}
      />
    </Row>
  );
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-2 pt-1">
      <h4 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/80">
        {title}
      </h4>
      {children}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

const ALIGNMENTS: { value: TextAlign; label: string; Icon: typeof AlignLeftIcon }[] = [
  { value: "left", label: "Align left", Icon: AlignLeftIcon },
  { value: "center", label: "Align centre", Icon: AlignCenterIcon },
  { value: "right", label: "Align right", Icon: AlignRightIcon },
];

/**
 * Quick placements, in the document's own units: `Transform::position` is in
 * half-canvas units with **+y up**, so "bottom third" is a negative y.
 *
 * These are the gesture a title actually needs. Fine control is the Transform
 * section above, which is shared with every other kind of clip and can
 * keyframe — deliberately not duplicated here.
 */
const PLACEMENTS: { label: string; position: [number, number] }[] = [
  { label: "Top", position: [0, 0.6] },
  { label: "Middle", position: [0, 0] },
  { label: "Lower third", position: [0, -0.5] },
];

export function TextInspector({
  project,
  segment,
  material,
}: {
  project: Project;
  segment: Segment;
  material: TextMaterial;
}) {
  const { draft, edit } = useTitleDraft(material);
  const fonts = useTextStore((s) => s.fonts);
  const fontsError = useTextStore((s) => s.fontsError);
  const loadFonts = useTextStore((s) => s.loadFonts);

  useEffect(() => {
    void loadFonts();
  }, [loadFonts]);

  const place = useCallback(
    (position: [number, number]) => {
      const after: Transform = { ...segment.transform, position };
      void runEdit(() =>
        timelineApply({
          type: "set_transform",
          segment_id: segment.id,
          before: segment.transform,
          after,
        }),
      );
    },
    [segment],
  );

  // The size range follows the canvas for the same reason the default does: a
  // ceiling of 200 px is a small title on a 4K canvas and an absurd one on a
  // 480p proxy.
  const shortEdge = Math.max(1, Math.min(project.canvas.width, project.canvas.height));

  return (
    <div data-slot="text-inspector" className="flex flex-col gap-2.5">
      <textarea
        aria-label="Title text"
        value={draft.content}
        rows={3}
        spellCheck={false}
        onChange={(event) => edit({ content: event.target.value })}
        placeholder="Type a title"
        className={cn(
          "w-full resize-y rounded-sm border border-input bg-background px-2 py-1.5",
          "text-[12px] leading-snug text-foreground placeholder:text-muted-foreground",
          "outline-none transition-[border-color,box-shadow]",
          "focus-visible:border-ring focus-visible:ring-[2px] focus-visible:ring-ring/40",
        )}
      />

      <Row label="Font">
        {fonts.length > 0 ? (
          <select
            aria-label="Font"
            value={draft.font_family}
            onChange={(event) => edit({ font_family: event.target.value })}
            className={cn(
              "h-[26px] w-full min-w-0 rounded-sm border border-input bg-background px-1.5",
              "text-[12px] text-foreground outline-none focus-visible:border-ring",
            )}
          >
            {/* The family the document names may not be installed here. Keeping
                it in the list is what stops opening someone else's project and
                silently rewriting their font to whatever sorted first. */}
            {fonts.includes(draft.font_family) ? null : (
              <option value={draft.font_family}>{draft.font_family} (not installed)</option>
            )}
            {fonts.map((family) => (
              <option key={family} value={family}>
                {family}
              </option>
            ))}
          </select>
        ) : (
          <Input
            aria-label="Font"
            value={draft.font_family}
            onChange={(event) => edit({ font_family: event.target.value })}
            placeholder={fontsError ? "Font family" : "Loading fonts…"}
          />
        )}
      </Row>

      <NumberField
        label="Size"
        value={draft.font_size}
        min={4}
        max={Math.max(64, Math.round(shortEdge * 0.6))}
        onChange={(font_size) => edit({ font_size })}
      />

      <Row label="Style">
        <Button
          variant="toggle"
          size="sm"
          aria-pressed={draft.bold}
          data-active={draft.bold}
          onClick={() => edit({ bold: !draft.bold })}
        >
          <BoldIcon />
          Bold
        </Button>
        <Button
          variant="toggle"
          size="sm"
          aria-pressed={draft.italic}
          data-active={draft.italic}
          onClick={() => edit({ italic: !draft.italic })}
        >
          <ItalicIcon />
          Italic
        </Button>
      </Row>

      <Row label="Align">
        {ALIGNMENTS.map(({ value, label, Icon }) => (
          <Button
            key={value}
            variant="toggle"
            size="icon-sm"
            aria-label={label}
            aria-pressed={draft.align === value}
            data-active={draft.align === value}
            onClick={() => edit({ align: value })}
          >
            <Icon />
          </Button>
        ))}
      </Row>

      <ColorField
        label="Colour"
        value={draft.color}
        withOpacity
        onChange={(color) => edit({ color })}
      />

      <Row label="Place">
        {PLACEMENTS.map((preset) => (
          <Button key={preset.label} size="sm" onClick={() => place(preset.position)}>
            {preset.label}
          </Button>
        ))}
      </Row>

      <Group title="Outline">
        <Row label="Outline">
          <Switch
            aria-label="Outline"
            checked={draft.stroke_width > 0}
            onCheckedChange={(on) =>
              edit({ stroke_width: on ? defaultOutlineWidth(draft.font_size) : 0 })
            }
          />
          <span className="text-[11px] text-muted-foreground">
            {draft.stroke_width > 0 ? "On" : "Off — white text on bright video is unreadable"}
          </span>
        </Row>
        {draft.stroke_width > 0 ? (
          <>
            <NumberField
              label="Width"
              value={draft.stroke_width}
              min={0}
              max={Math.max(4, Math.round(draft.font_size / 2))}
              onChange={(stroke_width) => edit({ stroke_width })}
            />
            <ColorField
              label="Outline colour"
              value={draft.stroke_color}
              onChange={(stroke_color) => edit({ stroke_color })}
            />
          </>
        ) : null}
      </Group>

      <Group title="Shadow">
        <Row label="Shadow">
          <Switch
            aria-label="Shadow"
            checked={draft.shadow !== null}
            onCheckedChange={(on) => edit({ shadow: on ? defaultShadow(draft.font_size) : null })}
          />
          <span className="text-[11px] text-muted-foreground">{draft.shadow ? "On" : "Off"}</span>
        </Row>
        {draft.shadow ? (
          <>
            <NumberField
              label="Offset X"
              value={draft.shadow.offset[0]}
              min={-Math.round(draft.font_size)}
              max={Math.round(draft.font_size)}
              onChange={(x) =>
                edit({
                  shadow: draft.shadow
                    ? { ...draft.shadow, offset: [x, draft.shadow.offset[1]] }
                    : null,
                })
              }
            />
            <NumberField
              label="Offset Y"
              value={draft.shadow.offset[1]}
              min={-Math.round(draft.font_size)}
              max={Math.round(draft.font_size)}
              onChange={(y) =>
                edit({
                  shadow: draft.shadow
                    ? { ...draft.shadow, offset: [draft.shadow.offset[0], y] }
                    : null,
                })
              }
            />
            <NumberField
              label="Blur"
              value={draft.shadow.blur}
              min={0}
              max={Math.max(8, Math.round(draft.font_size / 2))}
              onChange={(blur) => edit({ shadow: draft.shadow ? { ...draft.shadow, blur } : null })}
            />
            <ColorField
              label="Shadow colour"
              value={draft.shadow.color}
              withOpacity
              onChange={(color) =>
                edit({ shadow: draft.shadow ? { ...draft.shadow, color } : null })
              }
            />
          </>
        ) : null}
      </Group>

      <Group title="Background">
        <Row label="Box">
          <Switch
            aria-label="Background box"
            checked={draft.background !== null}
            onCheckedChange={(on) => edit({ background: on ? DEFAULT_BACKGROUND : null })}
          />
          <span className="text-[11px] text-muted-foreground">
            {draft.background ? "On" : "Off"}
          </span>
        </Row>
        {draft.background ? (
          <ColorField
            label="Box colour"
            value={draft.background}
            withOpacity
            onChange={(background) => edit({ background })}
          />
        ) : null}
      </Group>

      <p className="flex items-start gap-1.5 pt-1 text-[11px] leading-relaxed text-muted-foreground/70">
        <TypeIcon className="mt-px size-3 shrink-0" />
        <span>
          Titles trim, move, split and delete like any other clip. Fine positioning and keyframes
          are in Transform above.
        </span>
      </p>
    </div>
  );
}
