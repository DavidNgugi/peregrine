/** The browser's route, parsed from the address bar rather than kept only in React state. */

export type Route =
  | { readonly kind: "home" }
  | { readonly kind: "browse" }
  | { readonly kind: "myList" }
  | { readonly kind: "downloads" }
  | { readonly kind: "library" }
  | { readonly kind: "integrations" }
  | { readonly kind: "title"; readonly id: number }
  | { readonly kind: "watch"; readonly id: number };

/** The path a route is refreshed, shared, and backed into. */
export function routePath(route: Route): string {
  switch (route.kind) {
    case "home":
      return "/";
    case "browse":
      return "/browse";
    case "myList":
      return "/my-list";
    case "downloads":
      return "/downloads";
    case "library":
      return "/library";
    case "integrations":
      return "/integrations";
    case "title":
      return "/title/" + String(route.id);
    case "watch":
      return "/watch/" + String(route.id);
  }
}

/** Parse the current address into a route; an unknown path is the home screen. */
export function parseRoute(pathname: string): Route {
  const parts = pathname.split("/").filter((part) => part.length > 0);
  const [head, second] = parts;
  if (head === undefined) return { kind: "home" };
  if (head === "browse") return { kind: "browse" };
  if (head === "my-list") return { kind: "myList" };
  if (head === "downloads") return { kind: "downloads" };
  if (head === "library") return { kind: "library" };
  if (head === "integrations") return { kind: "integrations" };
  if ((head === "title" || head === "watch") && second !== undefined) {
    const id = Number(second);
    if (Number.isSafeInteger(id) && id > 0) {
      return head === "watch" ? { kind: "watch", id } : { kind: "title", id };
    }
  }
  return { kind: "home" };
}
