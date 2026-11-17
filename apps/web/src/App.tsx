// First, so errors during hydration are logged.
import './error-log';
import { lazy, Loading } from 'solid-js';
import { getRequestEvent } from '@solidjs/web';
import { isDevToolsPath } from './dev-tools/routes';
import { isManualPath } from './docs/manual-routes';
import { textPagePath, textPages } from './text/routes';
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
const Safety = lazy(() => import('./text/Safety'));
const TextPage = lazy(() => import('./text/TextPage'));
export default function App() {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  const textRoute = textPagePath(path);
  if (textRoute) {
    return (
      <Loading>
        {textRoute === '/safety' ? <Safety /> : <TextPage title={textPages[textRoute].heading} />}
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
