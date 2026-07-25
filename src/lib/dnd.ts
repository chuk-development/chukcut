/**
 * Drag payload shared between the media library and the timeline.
 *
 * The payload is the media file's absolute path rather than a library id, so
 * the timeline can resolve it against the project's material pool without
 * reaching into another module's store.
 */
export const MEDIA_DRAG_MIME = "application/x-chukcut-media";
