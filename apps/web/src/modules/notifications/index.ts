import { lazy } from "react";

export { notificationKeys, useMarkRead, useNotifications, usePreferences, useSavePreferences, useUnreadCount, useUnsubscribe, useUnsubscribeLink } from "./api";
export { badgeLabel, describe, issuePath } from "./model";
export { NotificationBell } from "./NotificationBell";

export const NotificationSettingsPage = lazy(() => import("./pages/NotificationSettingsPage"));
export const UnsubscribePage = lazy(() => import("./pages/UnsubscribePage"));
