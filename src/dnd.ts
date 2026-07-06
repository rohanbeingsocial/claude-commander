import { TouchBackend } from "react-dnd-touch-backend";

// Tauri intercepts native HTML5 drag events (for OS file-drop), which blocks react-dnd's
// default HTML5 backend. A mouse-enabled TouchBackend drives dragging with mouse events
// instead, so mosaic panes rearrange, explorer files drag onto tasks, AND Tauri's OS
// markdown file-drop all keep working. One provider at the App root serves everything.
export const DND_BACKEND = TouchBackend;
export const DND_OPTIONS = { enableMouseEvents: true, delayMouseStart: 0, delayTouchStart: 40 };
