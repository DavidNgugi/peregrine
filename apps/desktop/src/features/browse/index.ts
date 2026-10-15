// Browse's public surface: the screen the shell renders, and the data hook its tests (and any
// future caller) drive it with. The drawing is exported beside the data layer so a test can render
// the toolbar and the grid from synthetic DTOs, which is the split `DiscoverScreen`/`DiscoverView`
// already uses.
export { BrowseScreen, BrowseView } from "./components/BrowseScreen.js";
export type { BrowseViewProps } from "./components/BrowseScreen.js";
export { DEFAULT_FILTERS, useBrowse } from "./hooks/useBrowse.js";
export type { BrowseData, BrowseFilters } from "./hooks/useBrowse.js";
export { useBrowseGenres } from "./hooks/useBrowseGenres.js";
