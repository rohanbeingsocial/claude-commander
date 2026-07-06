import { useRef, useState, type CSSProperties } from "react";

/** A dropdown that renders fixed-positioned so it is never clipped by an ancestor's
 *  overflow (terminal cells and the task list both clip). Flips upward near the bottom
 *  of the viewport and caps its height so long menus scroll instead of overflowing. */
export function useDropdown() {
  const [open, setOpen] = useState(false);
  const [style, setStyle] = useState<CSSProperties>({});
  const btnRef = useRef<HTMLButtonElement>(null);

  const place = () => {
    const r = btnRef.current?.getBoundingClientRect();
    if (!r) return;
    const below = window.innerHeight - r.bottom;
    const s: CSSProperties = { right: Math.max(8, window.innerWidth - r.right) };
    if (below < 300 && r.top > below) {
      s.bottom = window.innerHeight - r.top + 4;
      s.maxHeight = r.top - 16;
    } else {
      s.top = r.bottom + 4;
      s.maxHeight = below - 16;
    }
    setStyle(s);
  };

  const toggle = (onOpen?: () => void) => {
    if (open) {
      setOpen(false);
      return;
    }
    place();
    onOpen?.();
    setOpen(true);
  };

  return { open, style, btnRef, toggle, close: () => setOpen(false) };
}
