/**
 * The browser APIs jsdom does not have, in the smallest shape the app needs.
 *
 * Every one of these exists because a component under test calls it, not
 * because it might one day: the preview paints to a 2D context, the panels
 * measure themselves with a `ResizeObserver`, and Radix's slider drives its
 * gesture off pointer capture. Stubbing them here keeps the shims in one file
 * instead of scattered `vi.stubGlobal` calls through the suite.
 */

/** The 2D calls the preview actually makes, in order, so a test can assert on them. */
export interface FakeContext2D {
  readonly calls: { op: string; args: unknown[] }[];
  /** Just the `drawImage` calls — the only one that means "a frame was painted". */
  readonly drawn: unknown[][];
}

const contexts = new WeakMap<HTMLCanvasElement, FakeContext2D>();

/** The recording context a canvas was given. Throws if it was never asked for one. */
export function contextOf(canvas: HTMLCanvasElement): FakeContext2D {
  const context = contexts.get(canvas);
  if (!context) throw new Error("this canvas never requested a 2D context");
  return context;
}

function makeContext2D(): CanvasRenderingContext2D & FakeContext2D {
  const calls: { op: string; args: unknown[] }[] = [];
  const drawn: unknown[][] = [];
  const record =
    (op: string) =>
    (...args: unknown[]) => {
      calls.push({ op, args });
      if (op === "drawImage") drawn.push(args);
    };
  const context = {
    calls,
    drawn,
    canvas: null,
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 1,
    font: "",
    textAlign: "start",
    textBaseline: "alphabetic",
    fillRect: record("fillRect"),
    strokeRect: record("strokeRect"),
    clearRect: record("clearRect"),
    beginPath: record("beginPath"),
    closePath: record("closePath"),
    moveTo: record("moveTo"),
    lineTo: record("lineTo"),
    stroke: record("stroke"),
    fill: record("fill"),
    fillText: record("fillText"),
    drawImage: record("drawImage"),
    save: record("save"),
    restore: record("restore"),
    translate: record("translate"),
    scale: record("scale"),
    measureText: () => ({ width: 0 }) as TextMetrics,
  };
  return context as unknown as CanvasRenderingContext2D & FakeContext2D;
}

class TestResizeObserver implements ResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

/** A pointer event that carries `pointerId`, which jsdom's `MouseEvent` does not. */
class TestPointerEvent extends MouseEvent {
  readonly pointerId: number;
  readonly pointerType: string;

  constructor(type: string, init: PointerEventInit = {}) {
    super(type, init);
    this.pointerId = init.pointerId ?? 1;
    this.pointerType = init.pointerType ?? "mouse";
  }
}

export function installDomShims(): void {
  if (!("ResizeObserver" in globalThis)) {
    globalThis.ResizeObserver = TestResizeObserver;
  }
  if (!("PointerEvent" in globalThis)) {
    globalThis.PointerEvent = TestPointerEvent as unknown as typeof PointerEvent;
  }

  // Radix's slider refuses to slide unless the element it pressed reports the
  // capture back, so this has to actually remember, not just be callable.
  const captured = new WeakMap<Element, Set<number>>();
  Element.prototype.setPointerCapture = function setPointerCapture(pointerId: number) {
    const ids = captured.get(this) ?? new Set<number>();
    ids.add(pointerId);
    captured.set(this, ids);
  };
  Element.prototype.releasePointerCapture = function releasePointerCapture(pointerId: number) {
    captured.get(this)?.delete(pointerId);
  };
  Element.prototype.hasPointerCapture = function hasPointerCapture(pointerId: number) {
    return captured.get(this)?.has(pointerId) ?? false;
  };
  Element.prototype.scrollIntoView = function scrollIntoView() {};

  HTMLCanvasElement.prototype.getContext = function getContext(
    this: HTMLCanvasElement,
    kind: string,
  ) {
    if (kind !== "2d") return null;
    const existing = contexts.get(this);
    if (existing) return existing as unknown as RenderingContext;
    const context = makeContext2D();
    contexts.set(this, context);
    return context as unknown as RenderingContext;
  } as HTMLCanvasElement["getContext"];
}

/**
 * Give an element a real box. jsdom reports zeros for everything, and a slider
 * whose track is zero pixels wide maps every pointer position to its minimum.
 */
export function stubRect(element: Element, rect: { left: number; width: number }): void {
  element.getBoundingClientRect = () =>
    ({
      x: rect.left,
      y: 0,
      left: rect.left,
      right: rect.left + rect.width,
      top: 0,
      bottom: 16,
      width: rect.width,
      height: 16,
      toJSON: () => ({}),
    }) as DOMRect;
}
