import { lazy, type ComponentType } from "react";
import { mountApp } from "./mount";
import "./channel/bootstrap";
import "./ui/native-window";

const AppLauncherPage = lazy(() => import("./pages/AppLauncherPage").then(module => ({ default: module.AppLauncherPage })));
const ChannelPreviewPage = lazy(() => import("./pages/ChannelPreviewPage").then(module => ({ default: module.ChannelPreviewPage })));
const ServerDomainsPage = lazy(() => import("./pages/ServerDomainsPage").then(module => ({ default: module.ServerDomainsPage })));
const previewPages: Record<string, ComponentType> = import.meta.env.DEV ? {
  "/__tooltip-preview": lazy(() => import("./pages/TooltipPreviewPage").then(module => ({ default: module.TooltipPreviewPage }))),
  "/__status-preview": lazy(() => import("./pages/StatusPreviewPage").then(module => ({ default: module.StatusPreviewPage }))),
  "/__playback-preview": lazy(() => import("./pages/PlaybackPreviewPage").then(module => ({ default: module.PlaybackPreviewPage }))),
} : {};
const PreviewPage = previewPages[location.pathname];
const domainRoot = location.pathname.match(/^\/d\/[0-9a-f-]{36}/i)?.[0] ?? "";
const pagePath = domainRoot ? location.pathname.slice(domainRoot.length) || "/" : location.pathname;

void mountApp(PreviewPage
  ? <PreviewPage />
  : pagePath === "/client" || pagePath === "/client/"
    ? <AppLauncherPage />
    : domainRoot
      ? <ChannelPreviewPage />
      : pagePath === "/"
        ? <ServerDomainsPage />
        : <ChannelPreviewPage />);
