// First, so errors during hydration are logged.
import './error-log';
import { Errored, lazy, Loading } from 'solid-js';
import { getRequestEvent, type JSX } from '@solidjs/web';
import { isDevToolsPath } from './dev-tools/routes';
import { isManualPath } from './docs/manual-routes';
import { textPagePath } from './text/routes';
import { CrashPage, ErrorPage } from './error-pages/ErrorPage';
import { servedErrorStatus } from './error-pages/routes';
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
const CustomSettingsEditorFeature = lazy(() => import('./features/custom-settings-editor/Page'));
const QrSharing = lazy(() => import('./text/QrSharing'));
const textRoutes = {
  '/safety': lazy(() => import('./text/Safety')),
  '/credits': lazy(() => import('./text/Credits')),
  '/releases': lazy(() => import('./text/Releases')),
  '/qr': QrSharing,
  '/recipe': QrSharing,
  '/settings': QrSharing,
};
/** A page that fails to render or load shows the error page instead of a blank one. */
function Guarded(props: { page: () => JSX.Element }) {
  return (
    <Errored fallback={(error) => <CrashPage error={error()} />}>
      <Loading>{props.page()}</Loading>
    </Errored>
  );
}
export default function App() {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  const error = servedErrorStatus(path);
  if (error) return <ErrorPage status={error} />;
  const textRoute = textPagePath(path);
  if (textRoute) {
    const TextPage = textRoutes[textRoute];
    return <Guarded page={() => <TextPage />} />;
  }
  if (ReleasePersonalNotePreview && path === '/__dev/release-personal-note') {
    return <Guarded page={() => <ReleasePersonalNotePreview />} />;
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
                : path === '/features/custom-settings-editor/' ||
                    path === '/features/custom-settings-editor'
                  ? CustomSettingsEditorFeature
                  : path.startsWith('/docs/sdk/')
                    ? SdkDocs
                    : isManualPath(path)
                      ? Manual
                      : path === '/'
                        ? Home
                        : undefined;
  return Page ? <Guarded page={() => <Page />} /> : <ErrorPage status={404} />;
}
