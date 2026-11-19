// First, so errors during hydration are logged.
import './error-log';
import { lazy, Loading } from 'solid-js';
import { getRequestEvent } from '@solidjs/web';
import { isDevToolsPath } from './dev-tools/routes';
import { isManualPath } from './docs/manual-routes';
import { textPagePath } from './text/routes';
const Home = lazy(() => import('./home/Page'));
const DevTools = lazy(() => import('./dev-tools/Page'));
const BugReport = lazy(() => import('./bug/Page'));
const Recovery = lazy(() => import('./recovery/Page'));
const Device = lazy(() => import('./device/Page'));
const ReleasePersonalNotePreview = import.meta.env.DEV
  ? lazy(() => import('./development/release-personal-note'))
  : undefined;
const SdkDocs = lazy(() => import('./docs/Page'));
const Manual = lazy(() => import('./docs/Manual'));
const DocsOverview = lazy(() => import('./docs/Overview'));
const DesignIndex = lazy(() => import('./docs/design/Index'));
const CustomSettingsEditor = lazy(() => import('./docs/design/custom-settings-editor/Page'));
const textRoutes = {
  '/safety': lazy(() => import('./text/Safety')),
  '/credits': lazy(() => import('./text/Credits')),
  '/releases': lazy(() => import('./text/Releases')),
};
export default function App() {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  const textRoute = textPagePath(path);
  if (textRoute) {
    const TextPage = textRoutes[textRoute];
    return (
      <Loading>
        <TextPage />
      </Loading>
    );
  }
  if (ReleasePersonalNotePreview && path === '/__dev/release-personal-note') {
    return (
      <Loading>
        <ReleasePersonalNotePreview />
      </Loading>
    );
  }
  const Page = isDevToolsPath(path)
    ? DevTools
    : path === '/bug/' || path === '/bug'
      ? BugReport
      : path === '/recovery/' || path === '/recovery'
        ? Recovery
        : path === '/device/' || path === '/device'
          ? Device
          : path === '/docs/' || path === '/docs'
            ? DocsOverview
            : path === '/docs/design/' || path === '/docs/design'
            ? DesignIndex
            : path.startsWith('/docs/design/custom-settings-editor')
            ? CustomSettingsEditor
            : path.startsWith('/docs/sdk/')
            ? SdkDocs
            : isManualPath(path)
              ? Manual
              : Home;
  return (
    <Loading>
      <Page />
    </Loading>
  );
}
