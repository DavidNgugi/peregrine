import { useEffect, useState } from "react";

/**
 * Whether the current viewport is the phone layout.
 *
 * The CSS breakpoint cannot also decide whether to render a control, and a collapsed section that
 * survives a rotation to desktop must not hide its content behind a button the desktop does not
 * draw. One hook keeps that decision in one place.
 */
export function useIsMobile(): boolean {
  const [mobile, setMobile] = useState(
    () => typeof window !== "undefined" && window.matchMedia("(max-width: 720px)").matches,
  );

  useEffect(() => {
    const query = window.matchMedia("(max-width: 720px)");
    const update = (): void => setMobile(query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);

  return mobile;
}
