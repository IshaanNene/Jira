import { lazy } from "react";

export { searchKeys, useDebounced, useQuickSearch, useSearch } from "./api";
export { Highlighted, hitPath, SearchResultRow, statusLabel } from "./components";

export const SearchPage = lazy(() => import("./pages/SearchPage"));
