import { useEffect, useRef } from "react";
import { attachTerm, fitTerm, focusTerm } from "../terminals";

/** Mounts a live terminal into the grid. Every mounted pane is visible and fits
 *  itself independently; the xterm instance survives unmount (see terminals.ts). */
export default function TerminalPane({ instanceId }: { instanceId: number }) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (ref.current) attachTerm(instanceId, ref.current);
  }, [instanceId]);

  useEffect(() => {
    if (!ref.current) return;
    const ro = new ResizeObserver(() => fitTerm(instanceId));
    ro.observe(ref.current);
    return () => ro.disconnect();
  }, [instanceId]);

  return <div ref={ref} className="term-host" onMouseDown={() => focusTerm(instanceId)} />;
}
