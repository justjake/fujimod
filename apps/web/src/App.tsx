// First, so errors during hydration are logged.
import './error-log';
import { lazy, Loading } from 'solid-js';
import { getRequestEvent } from '@solidjs/web';
import { isDevToolsPath } from './dev-tools/routes';
const Home = lazy(() => import('./home/Page'));
const DevTools = lazy(() => import('./dev-tools/Page'));
const BugReport = lazy(() => import('./bug/Page'));
const Recovery = lazy(() => import('./recovery/Page'));
export default function App() {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  const Page = isDevToolsPath(path)
    ? DevTools
    : path === '/bug/' || path === '/bug'
      ? BugReport
      : path === '/recovery/' || path === '/recovery'
        ? Recovery
        : Home;
  return (
    <Loading>
      <Page />
    </Loading>
  );
}
