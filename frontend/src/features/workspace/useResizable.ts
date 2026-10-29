/** Drag-to-resize a panel dimension, persisted in localStorage.
 *  `grow` is the side the panel grows toward when the handle moves:
 *  "right" for a left-docked panel, "left" for a right-docked panel, "up"
 *  for a bottom-docked panel. */
import { useCallback, useEffect, useRef, useState } from "react";

export function useResizable(
  storageKey: string,
  initial: number,
  min: number,
  max: number,
  grow: "right" | "left" | "up",
) {
  const [size, setSize] = useState(() => {
    const v = Number(localStorage.getItem(storageKey));
    return Number.isFinite(v) && v >= min && v <= max ? v : initial;
  });
  const drag = useRef<{ start: number; base: number } | null>(null);
  const sizeRef = useRef(size);
  sizeRef.current = size;

  const onMove = useCallback(
    (e: PointerEvent) => {
      const d = drag.current;
      if (!d) return;
      const pos = grow === "up" ? e.clientY : e.clientX;
      const delta = grow === "right" ? pos - d.start : d.start - pos;
      setSize(Math.min(max, Math.max(min, d.base + delta)));
    },
    [grow, min, max],
  );

  const onUp = useCallback(() => {
    if (!drag.current) return;
    drag.current = null;
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
    localStorage.setItem(storageKey, String(sizeRef.current));
  }, [storageKey]);

  useEffect(() => {
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
  }, [onMove, onUp]);

  const handleProps = {
    onPointerDown: (e: React.PointerEvent) => {
      e.preventDefault();
      drag.current = { start: grow === "up" ? e.clientY : e.clientX, base: sizeRef.current };
      document.body.style.cursor = grow === "up" ? "row-resize" : "col-resize";
      document.body.style.userSelect = "none";
    },
    onDoubleClick: () => {
      setSize(initial);
      localStorage.setItem(storageKey, String(initial));
    },
  };

  return { size, handleProps };
}
